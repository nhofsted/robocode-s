// SPDX-License-Identifier: EPL-2.0

//! Recording slots (spec §12.5): which battles may fill slots, slot ids and item keys, which
//! rounds of a battle fill which open slots, claiming them with a conditional update, the
//! S3 key paths of the recordings and the `recordings/done` check.

use crate::api::{BattleResult, RoundResult, SlotFailure};
use crate::config::Config;
use crate::ids::{Division, parse_vid36, vid36};
use crate::metrics::RumbleMetrics;
use crate::store::{
    SlotEntry, SlotItem, SlotStatus, Store, StoreError, StoreResult, User, UserState,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::collections::BTreeSet;
use std::fmt;

/// How long a claim waits for its upload (and the presigned PUT's validity), seconds.
pub const CLAIM_SECONDS: u64 = 3600;

/// The versions a slot item belongs to: a 1v1 pairing, or one melee version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// A 1v1 pairing, `lo < hi`.
    Pair { lo: u32, hi: u32 },
    /// A melee version.
    Version(u32),
}

/// Which rounds a slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// 1v1: won by the lower vid.
    A,
    /// 1v1: won by the higher vid.
    B,
    /// Melee: won by the version.
    Won,
    /// Melee: not won by the version.
    Lost,
}

impl Side {
    pub fn letter(self) -> char {
        match self {
            Side::A => 'a',
            Side::B => 'b',
            Side::Won => 'w',
            Side::Lost => 'l',
        }
    }
}

impl Group {
    pub fn division(self) -> Division {
        match self {
            Group::Pair { .. } => Division::OneVsOne,
            Group::Version(_) => Division::Melee,
        }
    }

    /// The slot item's key (§9.2): `<vidLo36>#<vidHi36>` or `<vid36>`.
    pub fn key(self) -> String {
        match self {
            Group::Pair { lo, hi } => format!("{}#{}", vid36(lo), vid36(hi)),
            Group::Version(vid) => vid36(vid),
        }
    }

    pub fn sides(self) -> [Side; 2] {
        match self {
            Group::Pair { .. } => [Side::A, Side::B],
            Group::Version(_) => [Side::Won, Side::Lost],
        }
    }

    /// Every slot of the item, side by side, in index order.
    pub fn slot_ids(self, per_side: u32) -> Vec<SlotId> {
        self.sides()
            .into_iter()
            .flat_map(|side| {
                (0..per_side).map(move |index| SlotId {
                    group: self,
                    side,
                    index,
                })
            })
            .collect()
    }

    /// The side a round with this winner (index into `vids`, the battle's robot order, or
    /// −1) belongs to, if any.
    fn side_of(self, vids: &[u32], winner: i32) -> Option<Side> {
        let winner = usize::try_from(winner)
            .ok()
            .and_then(|w| vids.get(w).copied());
        match self {
            Group::Pair { lo, hi } => match winner {
                Some(vid) if vid == lo => Some(Side::A),
                Some(vid) if vid == hi => Some(Side::B),
                _ => None,
            },
            Group::Version(vid) => Some(if winner == Some(vid) {
                Side::Won
            } else {
                Side::Lost
            }),
        }
    }
}

/// A slot: `1v1.<lo>.<hi>.<a|b>.<i>` or `melee.<vid>.<w|l>.<i>` (§9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId {
    pub group: Group,
    pub side: Side,
    pub index: u32,
}

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = self.side.letter();
        match self.group {
            Group::Pair { lo, hi } => write!(f, "1v1.{lo}.{hi}.{side}.{}", self.index),
            Group::Version(vid) => write!(f, "melee.{vid}.{side}.{}", self.index),
        }
    }
}

impl SlotId {
    /// The slot within its group: `<side letter>.<index>` (`a.0`, `w.3`).
    pub fn suffix(&self) -> String {
        format!("{}.{}", self.side.letter(), self.index)
    }

    /// The slot of `group` with this [`suffix`](SlotId::suffix).
    pub fn with_suffix(group: Group, suffix: &str) -> Option<SlotId> {
        let (letter, index) = suffix.split_once('.')?;
        let side = group
            .sides()
            .into_iter()
            .find(|s| letter.len() == 1 && letter.starts_with(s.letter()))?;
        Some(SlotId {
            group,
            side,
            index: index.parse().ok()?,
        })
    }

    pub fn parse(text: &str) -> Result<SlotId, String> {
        let bad = || format!("bad slot id {text:?}");
        let parts: Vec<&str> = text.split('.').collect();
        let number = |s: &str| -> Result<u32, String> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            s.parse().map_err(|_| bad())
        };
        let (group, side, index) = match parts.as_slice() {
            ["1v1", lo, hi, side, index] => {
                let (lo, hi) = (number(lo)?, number(hi)?);
                if lo >= hi {
                    return Err(bad());
                }
                let side = match *side {
                    "a" => Side::A,
                    "b" => Side::B,
                    _ => return Err(bad()),
                };
                (Group::Pair { lo, hi }, side, number(index)?)
            }
            ["melee", vid, side, index] => {
                let side = match *side {
                    "w" => Side::Won,
                    "l" => Side::Lost,
                    _ => return Err(bad()),
                };
                (Group::Version(number(vid)?), side, number(index)?)
            }
            _ => return Err(bad()),
        };
        Ok(SlotId { group, side, index })
    }
}

/// Whether a user's battles may fill slots (§12.5): reputation at least `rep.recorderMin`
/// and not quarantined (or banned).
pub fn eligible(user: &User, config: &Config) -> bool {
    user.reputation.state == UserState::New && user.reputation.rep >= config.rep.recorder_min
}

/// How many pairings of each kind (best, worst, problem bots) of every bot get slots (§12.5).
pub const PAIRS_PER_KIND: usize = 5;

/// The 1v1 matchups outside the `top` that get slots (§12.5), from the general 1v1 rumble's
/// metrics: for every bot its [`PAIRS_PER_KIND`] best pairings (highest score), worst
/// pairings (lowest score) and problem bots (most negative KNN-PBI), as `(lo, hi)` vid pairs,
/// sorted and without duplicates. At most 15 per bot. Ties go to the lower opponent vid.
pub fn slot_pairs(metrics: &RumbleMetrics) -> Vec<(u32, u32)> {
    let mut pairs: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (bot, row) in metrics.bots.iter().enumerate() {
        let vid = metrics.members[bot].vid;
        let mut seen: Vec<(u32, f64, f64)> = row
            .pairings
            .iter()
            .filter(|p| p.battles > 0 && p.opponent != bot)
            .map(|p| (metrics.members[p.opponent].vid, p.score, p.pbi))
            .collect();
        seen.sort_by_key(|&(opponent, ..)| opponent);
        let mut take = |key: &dyn Fn(&(u32, f64, f64)) -> f64, keep: &dyn Fn(f64) -> bool| {
            let mut ordered: Vec<&(u32, f64, f64)> = seen.iter().filter(|p| keep(key(p))).collect();
            ordered.sort_by(|a, b| key(a).total_cmp(&key(b)));
            for &&(opponent, ..) in ordered.iter().take(PAIRS_PER_KIND) {
                pairs.insert((vid.min(opponent), vid.max(opponent)));
            }
        };
        take(&|p| -p.1, &|_| true);
        take(&|p| p.1, &|_| true);
        take(&|p| p.2, &|pbi| pbi < 0.0);
    }
    pairs.into_iter().collect()
}

/// The slot items a counted `ok` battle may fill (§12.5): in 1v1 its pairing if either
/// version is in the plan's `top` or the pairing is one of the plan's `slotPairs`; in melee
/// each contestant in `top`.
pub fn groups(
    division: Division,
    vids: &[u32],
    top: &dyn Fn(u32) -> bool,
    pair: &dyn Fn(u32, u32) -> bool,
) -> Vec<Group> {
    match division {
        Division::OneVsOne => match vids {
            [a, b] if a != b && (top(*a) || top(*b) || pair(*a.min(b), *a.max(b))) => {
                vec![Group::Pair {
                    lo: *a.min(b),
                    hi: *a.max(b),
                }]
            }
            _ => vec![],
        },
        Division::Melee => {
            let mut groups: Vec<Group> = Vec::new();
            for &vid in vids {
                if top(vid) && !groups.contains(&Group::Version(vid)) {
                    groups.push(Group::Version(vid));
                }
            }
            groups
        }
    }
}

/// Whether a slot is open: absent, or claimed and the claim has expired.
pub fn is_open(entry: Option<&SlotEntry>, now: u64) -> bool {
    match entry {
        None => true,
        Some(entry) => entry.status == SlotStatus::Claimed && entry.claim_until <= now,
    }
}

/// Which rounds of a battle fill which open slots of a group: for each side, its open slots
/// in index order take that side's rounds (with a track) in round order. Returns
/// `(slot, round)` pairs.
pub fn pick(
    group: Group,
    vids: &[u32],
    rounds: &[RoundResult],
    item: Option<&SlotItem>,
    per_side: u32,
    now: u64,
) -> Vec<(SlotId, u32)> {
    let mut picks = Vec::new();
    for side in group.sides() {
        let open = (0..per_side)
            .map(|index| SlotId { group, side, index })
            .filter(|slot| is_open(item.and_then(|i| i.entries.get(&slot.to_string())), now));
        let side_rounds = rounds
            .iter()
            .enumerate()
            .filter(|(_, round)| {
                round.track.is_some() && group.side_of(vids, round.winner) == Some(side)
            })
            .map(|(r, _)| r as u32);
        picks.extend(open.zip(side_rounds));
    }
    picks
}

/// A claimed slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub slot: SlotId,
    pub entry: SlotEntry,
}

/// Claims the open slots a battle can fill (§12.5): per slot item, a read, the picks and
/// one conditional update (condition: the picked slots are as read). A lost race is retried
/// once with a fresh read. `index` is the battle's index in batch `batch_id`, `seed` its
/// seed, `vids` its robot order.
#[allow(clippy::too_many_arguments)]
pub fn claim_battle<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    engine: u32,
    batch_id: &str,
    index: u32,
    seed: i64,
    division: Division,
    vids: &[u32],
    result: &BattleResult,
    top: &dyn Fn(u32) -> bool,
    pair: &dyn Fn(u32, u32) -> bool,
    now: u64,
) -> StoreResult<Vec<Claim>> {
    let Some(rounds) = &result.rounds else {
        return Ok(Vec::new());
    };
    let per_side = config.slots.per_side;
    let mut claims = Vec::new();
    for group in groups(division, vids, top, pair) {
        let key = group.key();
        for _ in 0..2 {
            let item = store.get_slots(engine, division, &key)?;
            if item.as_ref().is_some_and(|item| item.full) {
                break;
            }
            let picks = pick(group, vids, rounds, item.as_ref(), per_side, now);
            if picks.is_empty() {
                break;
            }
            let mut expected = Vec::new();
            let mut set = Vec::new();
            let mut claimed = Vec::new();
            for (slot, round) in picks {
                let id = slot.to_string();
                let track = rounds[round as usize]
                    .track
                    .as_ref()
                    .expect("picked rounds have tracks");
                let Some(sha256_b64) = hex_to_base64(&track.sha256) else {
                    continue;
                };
                let entry = SlotEntry {
                    status: SlotStatus::Claimed,
                    batch: batch_id.to_string(),
                    index,
                    round,
                    sha256_b64,
                    size: track.size,
                    claim_until: now + CLAIM_SECONDS,
                    seed,
                    order: vids.to_vec(),
                };
                expected.push((
                    id.clone(),
                    item.as_ref().and_then(|i| i.entries.get(&id).cloned()),
                ));
                set.push((id, entry.clone()));
                claimed.push(Claim { slot, entry });
            }
            let full = item.as_ref().is_some_and(|i| i.full);
            match store.update_slots(engine, division, &key, &expected, &set, full) {
                Ok(()) => {
                    claims.extend(claimed);
                    break;
                }
                Err(StoreError::ConditionFailed) => continue,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(claims)
}

/// The S3 key of a slot's recording (§12.5). `version` gives a vid's bot id and jar hash
/// (hex). 1v1: `rec/e<E>/1v1/<idA>_<sha16A>/<idB>_<sha16B>/<a|b>-<i>.track` with A the bot
/// whose id sorts first (the lower vid on a tie) and `a` = A won, so the slot's side (by
/// vid) is translated; melee: `rec/e<E>/melee/<id>_<sha16>/<w|l>-<i>.track`.
pub fn recording_key(
    engine: u32,
    slot: &SlotId,
    version: &dyn Fn(u32) -> Option<(String, String)>,
) -> Option<String> {
    let dir = |vid: u32| -> Option<(String, String)> {
        let (id, sha) = version(vid)?;
        let sha16: String = sha.chars().take(16).collect();
        Some((id.clone(), format!("{id}_{sha16}")))
    };
    match slot.group {
        Group::Pair { lo, hi } => {
            let (lo_id, lo_dir) = dir(lo)?;
            let (hi_id, hi_dir) = dir(hi)?;
            let lo_first = lo_id <= hi_id;
            let (first, second) = if lo_first {
                (lo_dir, hi_dir)
            } else {
                (hi_dir, lo_dir)
            };
            let lo_won = slot.side == Side::A;
            let letter = if lo_won == lo_first { 'a' } else { 'b' };
            Some(format!(
                "rec/e{engine}/1v1/{first}/{second}/{letter}-{}.track",
                slot.index
            ))
        }
        Group::Version(vid) => {
            let (_, dir) = dir(vid)?;
            Some(format!(
                "rec/e{engine}/melee/{dir}/{}-{}.track",
                slot.side.letter(),
                slot.index
            ))
        }
    }
}

/// `recordings/done` for one slot (§5.4): the slot must be claimed by `batch_id`;
/// `head_sha256_b64` is the uploaded object's `ChecksumSHA256` from HeadObject (`None` if
/// the object is missing). A match marks the slot filled (and the item `full` when every
/// slot is). A claim that has passed its expiry but that nobody took over is still filled:
/// the upload is there and correct. Confirming a slot this batch already filled succeeds
/// again.
pub fn confirm<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    engine: u32,
    batch_id: &str,
    slot: &str,
    head_sha256_b64: Option<&str>,
) -> StoreResult<Result<(), SlotFailure>> {
    let Ok(id) = SlotId::parse(slot) else {
        return Ok(Err(SlotFailure::Missing));
    };
    let division = id.group.division();
    let key = id.group.key();
    let item = store.get_slots(engine, division, &key)?.unwrap_or_default();
    let Some(entry) = item.entries.get(slot).filter(|e| e.batch == batch_id) else {
        return Ok(Err(SlotFailure::Expired));
    };
    if entry.status == SlotStatus::Filled {
        return Ok(Ok(()));
    }
    let Some(head) = head_sha256_b64 else {
        return Ok(Err(SlotFailure::Missing));
    };
    if head != entry.sha256_b64 {
        return Ok(Err(SlotFailure::Checksum));
    }
    let filled = SlotEntry {
        status: SlotStatus::Filled,
        ..entry.clone()
    };
    let full = id
        .group
        .slot_ids(config.slots.per_side)
        .iter()
        .all(|other| {
            let other = other.to_string();
            other == slot
                || item
                    .entries
                    .get(&other)
                    .is_some_and(|e| e.status == SlotStatus::Filled)
        });
    match store.update_slots(
        engine,
        division,
        &key,
        &[(slot.to_string(), Some(entry.clone()))],
        &[(slot.to_string(), filled)],
        full,
    ) {
        Ok(()) => Ok(Ok(())),
        Err(StoreError::ConditionFailed) => Ok(Err(SlotFailure::Expired)),
        Err(e) => Err(e),
    }
}

/// Whether a text is 64 lowercase hex digits (a SHA-256 as the API writes it).
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A SHA-256 in hex as base64 (S3's `x-amz-checksum-sha256` form).
pub fn hex_to_base64(hex: &str) -> Option<String> {
    if !is_sha256_hex(hex) {
        return None;
    }
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let digit = |b: u8| (b as char).to_digit(16).unwrap() as u8;
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect();
    Some(STANDARD.encode(bytes))
}

/// Parses the vid part of a slot item key (for tools).
pub fn parse_key(division: Division, key: &str) -> Result<Group, String> {
    match division {
        Division::OneVsOne => {
            let (lo, hi) = key
                .split_once('#')
                .ok_or_else(|| format!("bad slot key {key:?}"))?;
            Ok(Group::Pair {
                lo: parse_vid36(lo)?,
                hi: parse_vid36(hi)?,
            })
        }
        Division::Melee => Ok(Group::Version(parse_vid36(key)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{BattleStatus, TrackClaim};
    use crate::store::{MemStore, Reputation};

    const NOW: u64 = 1_792_843_200;

    fn no_pair(_: u32, _: u32) -> bool {
        false
    }

    fn sha(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn round(winner: i32, n: u8) -> RoundResult {
        RoundResult {
            turns: 900,
            winner,
            firsts: vec![],
            scores: vec![],
            hash: sha(0),
            track: Some(TrackClaim {
                sha256: sha(n),
                size: 3000 + u64::from(n),
            }),
        }
    }

    fn battle(winners: &[i32]) -> BattleResult {
        BattleResult {
            index: 0,
            status: BattleStatus::Ok,
            duration_ms: None,
            battle_hash: Some(sha(1)),
            robots: None,
            rounds: Some(
                winners
                    .iter()
                    .enumerate()
                    .map(|(r, &w)| round(w, r as u8))
                    .collect(),
            ),
            failure: None,
        }
    }

    #[test]
    fn ids_and_keys() {
        let pair = Group::Pair { lo: 77, hi: 812 };
        let slot = SlotId {
            group: pair,
            side: Side::A,
            index: 2,
        };
        assert_eq!(slot.to_string(), "1v1.77.812.a.2");
        assert_eq!(SlotId::parse("1v1.77.812.a.2"), Ok(slot));
        assert_eq!(pair.key(), "25#mk");
        assert_eq!(parse_key(Division::OneVsOne, "25#mk"), Ok(pair));
        let melee = SlotId {
            group: Group::Version(812),
            side: Side::Lost,
            index: 4,
        };
        assert_eq!(melee.to_string(), "melee.812.l.4");
        assert_eq!(SlotId::parse("melee.812.l.4"), Ok(melee));
        assert_eq!(Group::Version(812).key(), "mk");
        for bad in [
            "1v1.812.77.a.2",
            "1v1.1.2.w.0",
            "melee.1.a.0",
            "teams.1.w.0",
            "1v1.1.2.a",
            "1v1.1.2.a.-1",
        ] {
            assert!(SlotId::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(pair.slot_ids(5).len(), 10);
    }

    #[test]
    fn eligibility() {
        let config = Config::default();
        let mut user = User::default();
        user.reputation.rep = 19;
        assert!(!eligible(&user, &config));
        user.reputation.rep = 20;
        assert!(eligible(&user, &config));
        user.reputation.state = UserState::Quarantined;
        assert!(!eligible(&user, &config));

        let top = |vid: u32| vid == 5;
        assert_eq!(
            groups(Division::OneVsOne, &[9, 5], &top, &no_pair),
            [Group::Pair { lo: 5, hi: 9 }]
        );
        assert!(groups(Division::OneVsOne, &[9, 6], &top, &no_pair).is_empty());
        assert_eq!(
            groups(Division::Melee, &[1, 5, 2, 3], &top, &no_pair),
            [Group::Version(5)]
        );
        assert!(groups(Division::Melee, &[1, 2], &top, &no_pair).is_empty());

        // Slot pairs make a 1v1 pairing eligible in either order, never a melee version.
        let pairs = |lo: u32, hi: u32| (lo, hi) == (6, 9);
        for order in [[9, 6], [6, 9]] {
            assert_eq!(
                groups(Division::OneVsOne, &order, &top, &pairs),
                [Group::Pair { lo: 6, hi: 9 }]
            );
        }
        assert!(groups(Division::OneVsOne, &[9, 7], &top, &pairs).is_empty());
        assert!(groups(Division::Melee, &[6, 9], &top, &pairs).is_empty());
    }

    #[test]
    fn pairs_of_best_worst_and_problem_bots() {
        use crate::metrics::{Member, compute};
        use crate::record::{Entry, Record};
        use crate::state::State;
        // 12 bots; bot 1 scores 50 + j against bot j, everyone else 50:50.
        let mut state = State::new(1, Division::OneVsOne, 0);
        let mut fight = |a: u32, b: u32, sa: f32| {
            let entry = |vid, score| Entry {
                vid,
                score,
                firsts: 5,
            };
            let record = Record {
                division: Division::OneVsOne,
                weight: 1,
                uid: "g1".to_string(),
                contestants: vec![entry(a, sa), entry(b, 100.0 - sa)],
                failed: None,
                trusted: false,
            };
            state.fold(&record, 1);
        };
        for a in 1..=12u32 {
            for b in a + 1..=12 {
                fight(a, b, if a == 1 { 50.0 + b as f32 } else { 50.0 });
            }
        }
        let members = (1..=12u32)
            .map(|vid| Member {
                vid,
                id: format!("b.B{vid}"),
                version: "1".into(),
                author: "x".into(),
                country: None,
                rank_week_ago: None,
            })
            .collect();
        let metrics = compute(members, &state);
        let pairs = slot_pairs(&metrics);
        assert!(pairs.windows(2).all(|w| w[0] < w[1]), "sorted, unique");
        assert!(pairs.iter().all(|&(lo, hi)| lo < hi));
        // Bot 1's best (12..8) and worst (2..6) opponents.
        for other in [12, 11, 10, 9, 8, 2, 3, 4, 5, 6] {
            assert!(pairs.contains(&(1, other)), "(1, {other})");
        }
        for vid in 1..=12 {
            let n = pairs.iter().filter(|p| p.0 == vid || p.1 == vid).count();
            assert!(n >= 5, "bot {vid} has {n}");
        }
        assert!(pairs.len() < 66, "not every pairing: {}", pairs.len());
    }

    #[test]
    fn picking_rounds() {
        let pair = Group::Pair { lo: 5, hi: 9 };
        // Robot order [9, 5]: winner 1 is the lower vid (side a).
        let b = battle(&[1, 0, 1, -1, 1, 1, 1, 1, 0]);
        let picks = pick(pair, &[9, 5], b.rounds.as_ref().unwrap(), None, 5, NOW);
        let text: Vec<(String, u32)> = picks.iter().map(|(s, r)| (s.to_string(), *r)).collect();
        assert_eq!(
            text,
            [
                ("1v1.5.9.a.0".to_string(), 0),
                ("1v1.5.9.a.1".to_string(), 2),
                ("1v1.5.9.a.2".to_string(), 4),
                ("1v1.5.9.a.3".to_string(), 5),
                ("1v1.5.9.a.4".to_string(), 6),
                ("1v1.5.9.b.0".to_string(), 1),
                ("1v1.5.9.b.1".to_string(), 8),
            ]
        );
        // Melee: won and not won rounds of version 5 (robot 1).
        let b = battle(&[1, 0, 2, -1]);
        let picks = pick(
            Group::Version(5),
            &[9, 5, 3],
            b.rounds.as_ref().unwrap(),
            None,
            5,
            NOW,
        );
        let text: Vec<String> = picks.iter().map(|(s, r)| format!("{s}@{r}")).collect();
        assert_eq!(
            text,
            [
                "melee.5.w.0@0",
                "melee.5.l.0@1",
                "melee.5.l.1@2",
                "melee.5.l.2@3"
            ]
        );
    }

    fn recorder() -> User {
        User {
            uid: "g1".into(),
            reputation: Reputation {
                rep: 30,
                ..Reputation::default()
            },
            ..User::default()
        }
    }

    #[test]
    fn claiming_and_filling() {
        let store = MemStore::new();
        let mut config = Config::default();
        config.slots.per_side = 2;
        assert!(eligible(&recorder(), &config));
        let top = |vid: u32| vid == 5;
        let b = battle(&[0, 0, 1, 0, 1, 1]);
        let claims = claim_battle(
            &store,
            &config,
            1,
            "b1",
            3,
            42,
            Division::OneVsOne,
            &[5, 9],
            &b,
            &top,
            &no_pair,
            NOW,
        )
        .unwrap();
        let ids: Vec<String> = claims.iter().map(|c| c.slot.to_string()).collect();
        assert_eq!(
            ids,
            ["1v1.5.9.a.0", "1v1.5.9.a.1", "1v1.5.9.b.0", "1v1.5.9.b.1"]
        );
        assert_eq!(claims[1].entry.round, 1);
        assert_eq!(claims[1].entry.index, 3);
        assert_eq!(claims[1].entry.seed, 42);
        assert_eq!(claims[1].entry.claim_until, NOW + CLAIM_SECONDS);
        assert_eq!(claims[1].entry.sha256_b64, hex_to_base64(&sha(1)).unwrap());

        // A second battle finds every slot claimed.
        let again = claim_battle(
            &store,
            &config,
            1,
            "b2",
            0,
            1,
            Division::OneVsOne,
            &[5, 9],
            &b,
            &top,
            &no_pair,
            NOW,
        )
        .unwrap();
        assert!(again.is_empty());

        // Filling: wrong batch, missing object, wrong checksum, then right.
        let slot = "1v1.5.9.a.1";
        let right = claims[1].entry.sha256_b64.clone();
        assert_eq!(
            confirm(&store, &config, 1, "b2", slot, Some(&right)).unwrap(),
            Err(SlotFailure::Expired)
        );
        assert_eq!(
            confirm(&store, &config, 1, "b1", slot, None).unwrap(),
            Err(SlotFailure::Missing)
        );
        assert_eq!(
            confirm(&store, &config, 1, "b1", slot, Some("x")).unwrap(),
            Err(SlotFailure::Checksum)
        );
        assert_eq!(
            confirm(&store, &config, 1, "b1", slot, Some(&right)).unwrap(),
            Ok(())
        );
        assert_eq!(
            confirm(&store, &config, 1, "b1", slot, Some(&right)).unwrap(),
            Ok(()),
            "idempotent"
        );
        assert!(
            !store
                .get_slots(1, Division::OneVsOne, "5#9")
                .unwrap()
                .unwrap()
                .full
        );

        // Claims expire: another batch takes over the unfilled ones, never the filled one.
        let later = NOW + CLAIM_SECONDS;
        let taken = claim_battle(
            &store,
            &config,
            1,
            "b3",
            0,
            7,
            Division::OneVsOne,
            &[5, 9],
            &b,
            &top,
            &no_pair,
            later,
        )
        .unwrap();
        let ids: Vec<String> = taken.iter().map(|c| c.slot.to_string()).collect();
        assert_eq!(ids, ["1v1.5.9.a.0", "1v1.5.9.b.0", "1v1.5.9.b.1"]);
        // Every claim records the battle's robot order.
        assert!(taken.iter().all(|c| c.entry.order == [5, 9]));
        assert_eq!(
            confirm(
                &store,
                &config,
                1,
                "b1",
                "1v1.5.9.a.0",
                Some(&claims[0].entry.sha256_b64)
            )
            .unwrap(),
            Err(SlotFailure::Expired)
        );
        for claim in &taken {
            let id = claim.slot.to_string();
            assert_eq!(
                confirm(&store, &config, 1, "b3", &id, Some(&claim.entry.sha256_b64)).unwrap(),
                Ok(())
            );
        }
        let item = store
            .get_slots(1, Division::OneVsOne, "5#9")
            .unwrap()
            .unwrap();
        assert!(item.full);
        assert!(
            item.entries
                .values()
                .all(|e| e.status == SlotStatus::Filled)
        );
        // A full item is not even considered.
        let none = claim_battle(
            &store,
            &config,
            1,
            "b4",
            0,
            7,
            Division::OneVsOne,
            &[5, 9],
            &b,
            &top,
            &no_pair,
            later + CLAIM_SECONDS,
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn reopening_slots() {
        let store = MemStore::new();
        let entry = |status, order: Vec<u32>| SlotEntry {
            status,
            batch: "b1".into(),
            index: 0,
            round: 0,
            sha256_b64: "x".into(),
            size: 1,
            claim_until: 0,
            seed: 0,
            order,
        };
        let old = entry(SlotStatus::Filled, vec![]);
        let new = entry(SlotStatus::Filled, vec![5, 9]);
        let set = vec![
            ("1v1.5.9.a.0".to_string(), old.clone()),
            ("1v1.5.9.a.1".to_string(), new),
        ];
        let expected: Vec<(String, Option<SlotEntry>)> =
            set.iter().map(|(s, _)| (s.clone(), None)).collect();
        store
            .update_slots(1, Division::OneVsOne, "5#9", &expected, &set, true)
            .unwrap();
        let wrong = entry(SlotStatus::Claimed, vec![]);
        assert_eq!(
            store.remove_slots(
                1,
                Division::OneVsOne,
                "5#9",
                &[("1v1.5.9.a.0".into(), wrong)]
            ),
            Err(StoreError::ConditionFailed)
        );
        store
            .remove_slots(1, Division::OneVsOne, "5#9", &[("1v1.5.9.a.0".into(), old)])
            .unwrap();
        let item = store
            .get_slots(1, Division::OneVsOne, "5#9")
            .unwrap()
            .unwrap();
        assert_eq!(item.entries.keys().collect::<Vec<_>>(), ["1v1.5.9.a.1"]);
        assert!(!item.full);
    }

    #[test]
    fn racing_claims() {
        let store = MemStore::new();
        let config = Config::default();
        let pair = Group::Pair { lo: 5, hi: 9 };
        let b = battle(&[0, 1]);
        let rounds = b.rounds.as_ref().unwrap();
        // Both batches read the empty item, then both try to claim the same slots.
        let picks = pick(pair, &[5, 9], rounds, None, 5, NOW);
        let entry = |batch: &str| SlotEntry {
            status: SlotStatus::Claimed,
            batch: batch.to_string(),
            index: 0,
            round: 0,
            sha256_b64: "x".into(),
            size: 1,
            claim_until: NOW + CLAIM_SECONDS,
            seed: 0,
            order: vec![5, 9],
        };
        let expected: Vec<(String, Option<SlotEntry>)> =
            picks.iter().map(|(s, _)| (s.to_string(), None)).collect();
        let set = |batch: &str| -> Vec<(String, SlotEntry)> {
            picks
                .iter()
                .map(|(s, _)| (s.to_string(), entry(batch)))
                .collect()
        };
        store
            .update_slots(
                1,
                Division::OneVsOne,
                &pair.key(),
                &expected,
                &set("b1"),
                false,
            )
            .unwrap();
        assert_eq!(
            store.update_slots(
                1,
                Division::OneVsOne,
                &pair.key(),
                &expected,
                &set("b2"),
                false
            ),
            Err(StoreError::ConditionFailed)
        );
        let item = store
            .get_slots(1, Division::OneVsOne, &pair.key())
            .unwrap()
            .unwrap();
        assert!(item.entries.values().all(|e| e.batch == "b1"));
        // claim_battle's retry re-reads and picks the slots still open.
        let b2 = battle(&[0, 0]);
        let claims = claim_battle(
            &store,
            &config,
            1,
            "b2",
            0,
            0,
            Division::OneVsOne,
            &[5, 9],
            &b2,
            &|_| true,
            &no_pair,
            NOW,
        )
        .unwrap();
        let ids: Vec<String> = claims.iter().map(|c| c.slot.to_string()).collect();
        assert_eq!(ids, ["1v1.5.9.a.1", "1v1.5.9.a.2"]);
    }

    #[test]
    fn recording_keys() {
        let versions = |vid: u32| -> Option<(String, String)> {
            match vid {
                5 => Some(("voidious.Diamond".into(), "ab".repeat(32))),
                9 => Some(("jk.mega.DrussGT".into(), "cd".repeat(32))),
                _ => None,
            }
        };
        // jk.mega.DrussGT (vid 9) sorts first: it is A, so the lower vid winning is `b`.
        let slot = SlotId::parse("1v1.5.9.a.3").unwrap();
        assert_eq!(
            recording_key(1, &slot, &versions).unwrap(),
            "rec/e1/1v1/jk.mega.DrussGT_cdcdcdcdcdcdcdcd/voidious.Diamond_abababababababab/b-3.track"
        );
        let slot = SlotId::parse("1v1.5.9.b.0").unwrap();
        assert!(
            recording_key(1, &slot, &versions)
                .unwrap()
                .ends_with("/a-0.track")
        );
        let slot = SlotId::parse("melee.5.w.1").unwrap();
        assert_eq!(
            recording_key(2, &slot, &versions).unwrap(),
            "rec/e2/melee/voidious.Diamond_abababababababab/w-1.track"
        );
        assert!(recording_key(1, &SlotId::parse("melee.6.w.1").unwrap(), &versions).is_none());
    }

    #[test]
    fn checksums() {
        assert_eq!(
            hex_to_base64("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap(),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
        assert!(hex_to_base64("E3B0").is_none());
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(is_sha256_hex(&"a".repeat(64)));
    }
}
