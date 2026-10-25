// SPDX-License-Identifier: EPL-2.0

//! `POST results` (spec §5.4): validation of each battle and the whole post: batch token,
//! engine and deadline checks, the idempotent repost, records into one log item in one
//! transaction with the batch marker and the user's counters, battles picked for
//! verification (§11.2), verification and probation work coming back (§11.3, §12.3), and
//! recording slots claimed (§12.5).

use crate::api::{
    ApiError, BattleKind, BattleResult, BattleStatus, BotColors, ErrorCode, Rejected,
    ResultsRequest, ResultsResponse, UploadSlot, UserSummary,
};
use crate::config::Config;
use crate::ids::Division;
use crate::record::Record;
use crate::slots::{self, Claim, SlotId, is_sha256_hex};
use crate::store::{
    BatchMarker, LOG_TTL, LogItem, MARKER_TTL, ResultsTransaction, Run, SlotEntry, SlotStatus,
    Store, StoreError, StoreResult, User, UserState, VERIFY_TTL, VerifyItem, VerifyKind, hour_key,
};
use crate::token::{self, BatchBattle, BatchPayload, Keys, TokenError};
use crate::verify::{self, Outcome, is_verifier, tier, verify_rate};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The `error` of a rejected battle.
pub const INVALID: &str = "invalid";

/// A presigned S3 PUT, as the Lambda's presigner makes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Presigned {
    pub url: String,
    /// The headers the client must send exactly (`Content-Length`, `x-amz-checksum-sha256`,
    /// `Content-Type`, `Cache-Control`).
    pub headers: BTreeMap<String, String>,
    /// RFC 3339.
    pub expires: String,
}

/// Presigning is I/O-free in principle but needs the AWS credentials, so the Lambda
/// implements it: a PUT of `key` in the jars bucket, valid [`slots::CLAIM_SECONDS`], with
/// `Content-Length: size`, `x-amz-checksum-sha256: sha256_b64` and the immutable
/// `Cache-Control` signed (§12.5).
pub trait Presigner {
    fn put_recording(&self, key: &str, sha256_b64: &str, size: u64, now: u64) -> Presigned;
}

/// What a results post needs besides the store and the request.
pub struct ResultsContext<'a> {
    pub config: &'a Config,
    pub keys: &'a Keys,
    /// Whether a version is in the current plan's `top` of a division (§10.5).
    pub top: &'a dyn Fn(Division, u32) -> bool,
    /// Whether a 1v1 pairing (lower vid, higher vid) is in the plan's `slotPairs`.
    pub slot_pair: &'a dyn Fn(Division, u32, u32) -> bool,
    pub presigner: &'a dyn Presigner,
}

/// The result of a post: the response body and lines the Lambda must log (`ALERT ...`).
#[derive(Clone, Debug, PartialEq)]
pub struct ResultsOutcome {
    pub response: ResultsResponse,
    pub log: Vec<String>,
}

/// A deterministic stream of random numbers from the caller's 32 random bytes: SHA-256 of
/// the bytes and a counter.
pub struct Random {
    seed: [u8; 32],
    counter: u64,
}

impl Random {
    pub fn new(seed: &[u8; 32]) -> Random {
        Random {
            seed: *seed,
            counter: 0,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut hasher = Sha256::new();
        hasher.update(self.seed);
        hasher.update(self.counter.to_le_bytes());
        self.counter += 1;
        u64::from_le_bytes(hasher.finalize()[..8].try_into().unwrap())
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
}

/// Validates one battle against its assignment (§5.4). `ok` battles: `robots` for each of
/// the battle's bots, `rounds` for each of its rounds, finite non-negative numbers, each
/// robot's round scores summing to its score within 1e-6 relative (absolute below a score of
/// 1), `firsts` equal to the rounds it won, a battle hash and round hashes of 64 lowercase
/// hex digits, track hashes likewise and track sizes 1..=`slots.maxBytes`. `robotFailed`
/// battles name a robot of the battle. `notRun` and `error` battles are always valid.
pub fn validate(
    battle: &BattleResult,
    assigned: &BatchBattle,
    config: &Config,
) -> Result<(), String> {
    let n = assigned.vids.len();
    match battle.status {
        BattleStatus::NotRun | BattleStatus::Error => Ok(()),
        BattleStatus::RobotFailed => {
            let failure = battle.failure.as_ref().ok_or("failure missing")?;
            if failure.robot as usize >= n {
                return Err(format!("failure.robot: {} of {n} robots", failure.robot));
            }
            Ok(())
        }
        BattleStatus::Ok => {
            let hash = battle.battle_hash.as_deref().ok_or("battleHash missing")?;
            if !is_sha256_hex(hash) {
                return Err("battleHash: not 64 lowercase hex digits".to_string());
            }
            let robots = battle.robots.as_ref().ok_or("robots missing")?;
            if robots.len() != n {
                return Err(format!("robots: {} of {n}", robots.len()));
            }
            let rounds = battle.rounds.as_ref().ok_or("rounds missing")?;
            if rounds.len() != assigned.rounds as usize {
                return Err(format!("rounds: {} of {}", rounds.len(), assigned.rounds));
            }
            for (r, robot) in robots.iter().enumerate() {
                for (name, value) in [
                    ("score", robot.score),
                    ("survival", robot.survival),
                    ("lastSurvivorBonus", robot.last_survivor_bonus),
                    ("bulletDamage", robot.bullet_damage),
                    ("bulletDamageBonus", robot.bullet_damage_bonus),
                    ("ramDamage", robot.ram_damage),
                    ("ramDamageBonus", robot.ram_damage_bonus),
                ] {
                    if !value.is_finite() || value < 0.0 {
                        return Err(format!("robots[{r}].{name}: {value}"));
                    }
                }
            }
            let mut sums = vec![0.0f64; n];
            let mut wins = vec![0u32; n];
            for (i, round) in rounds.iter().enumerate() {
                if round.scores.len() != n {
                    return Err(format!("rounds[{i}].scores: {} of {n}", round.scores.len()));
                }
                for (r, &score) in round.scores.iter().enumerate() {
                    if !score.is_finite() || score < 0.0 {
                        return Err(format!("rounds[{i}].scores[{r}]: {score}"));
                    }
                    sums[r] += score;
                }
                if round.firsts.is_empty() {
                    match usize::try_from(round.winner) {
                        Ok(w) if w < n => wins[w] += 1,
                        Err(_) if round.winner == -1 => {}
                        _ => return Err(format!("rounds[{i}].winner: {}", round.winner)),
                    }
                } else {
                    for (k, &f) in round.firsts.iter().enumerate() {
                        if f as usize >= n || round.firsts[..k].contains(&f) {
                            return Err(format!("rounds[{i}].firsts: {:?}", round.firsts));
                        }
                        wins[f as usize] += 1;
                    }
                    let single = match round.firsts[..] {
                        [only] => only as i32,
                        _ => -1,
                    };
                    if round.winner != single {
                        return Err(format!(
                            "rounds[{i}].winner: {} with firsts {:?}",
                            round.winner, round.firsts
                        ));
                    }
                }
                if !is_sha256_hex(&round.hash) {
                    return Err(format!("rounds[{i}].hash: not 64 lowercase hex digits"));
                }
                if let Some(track) = &round.track {
                    if !is_sha256_hex(&track.sha256) {
                        return Err(format!(
                            "rounds[{i}].track.sha256: not 64 lowercase hex digits"
                        ));
                    }
                    if track.size == 0 || track.size > config.slots.max_bytes {
                        return Err(format!(
                            "rounds[{i}].track.size: {} not in 1..{}",
                            track.size, config.slots.max_bytes
                        ));
                    }
                }
            }
            for (r, robot) in robots.iter().enumerate() {
                if (sums[r] - robot.score).abs() > 1e-6 * robot.score.abs().max(1.0) {
                    return Err(format!(
                        "robots[{r}].score: {} but its rounds sum to {}",
                        robot.score, sums[r]
                    ));
                }
                if robot.firsts != wins[r] {
                    return Err(format!(
                        "robots[{r}].firsts: {} but it won {} rounds",
                        robot.firsts, wins[r]
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Validates every battle of a post (§5.4): its index must be in the batch and appear at
/// most once (later copies are rejected), then [`validate`]. Returns the accepted indices
/// (ascending) and the rejections (in post order).
pub fn validate_all(
    request: &ResultsRequest,
    payload: &BatchPayload,
    config: &Config,
) -> (Vec<u32>, Vec<Rejected>) {
    let mut seen = HashSet::new();
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    let reject = |index: u32, message: String| Rejected {
        index,
        error: INVALID.to_string(),
        message,
    };
    for battle in &request.battles {
        let index = battle.index;
        let Some(assigned) = payload.b.get(index as usize) else {
            rejected.push(reject(index, format!("index {index} is not in the batch")));
            continue;
        };
        if !seen.insert(index) {
            rejected.push(reject(index, format!("index {index} twice")));
            continue;
        }
        match validate(battle, assigned, config) {
            Ok(()) => accepted.push(index),
            Err(message) => rejected.push(reject(index, message)),
        }
    }
    accepted.sort_unstable();
    (accepted, rejected)
}

fn summary(user: &User, added: u64, config: &Config) -> UserSummary {
    UserSummary {
        battles: user.battles + added,
        reputation: i64::from(user.reputation.rep),
        tier: tier(&user.reputation, config),
    }
}

fn engine_mismatch(theirs: u32, ours: u32) -> ApiError {
    ApiError::new(
        ErrorCode::EngineMismatch,
        format!("the rumble ranks engine version {ours}; this batch is for engine {theirs}"),
    )
}

/// Handles a results post of `user` (already authenticated) at `now_ms` (milliseconds),
/// with 32 fresh random bytes (§5.4, §9.3, §11.2, §11.3, §12.3, §12.5):
///
/// 1. The batch token (signature, then deadline: 409 `expired`), the engine of the request
///    and of the token (409 `engine_mismatch`), the token's user (403).
/// 2. A batch already posted returns the stored `accepted`/`rejected` with `duplicate: true`
///    and fresh URLs for the slots it claimed and hasn't filled.
/// 3. Validation; records of accepted `ok` and `robotFailed` battles of kind `normal` and
///    `probation` (with the `trusted` flag: reputation ≥ `rep.verifierMin`) go into one log
///    item, held if the user is quarantined; one transaction writes it with the marker and
///    the user's counters.
/// 4. Then: verification items for picked battles (rate r; all of a quarantined user's),
///    probation items ended, verification runs resolved, unrun verification and probation
///    work given back to the queue, slots claimed and presigned.
///
/// Step 4 runs after the transaction: if it fails half-way, a repost answers as a duplicate
/// and what was not done stays undone (an unverified battle, unclaimed slots, a claim that
/// expires back into the queue). Nothing is ever counted twice.
pub fn apply_results<S: Store + ?Sized>(
    store: &S,
    ctx: &ResultsContext,
    user: &User,
    request: &ResultsRequest,
    now_ms: u64,
    random: &[u8; 32],
) -> Result<ResultsOutcome, ApiError> {
    let config = ctx.config;
    let engine = config.engine_version;
    let now = now_ms / 1000;
    if user.reputation.state == UserState::Banned {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "this account is banned",
        ));
    }
    if request.engine_version != engine {
        return Err(engine_mismatch(request.engine_version, engine));
    }
    let payload = match token::verify(&request.batch, ctx.keys, now) {
        Ok(payload) => payload,
        Err(TokenError::Expired) => {
            return Err(ApiError::new(
                ErrorCode::Expired,
                "the batch is past its deadline",
            ));
        }
        Err(e) => return Err(ApiError::new(ErrorCode::BadRequest, format!("batch: {e}"))),
    };
    if payload.e != engine {
        return Err(engine_mismatch(payload.e, engine));
    }
    if payload.uid != user.uid {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "the batch was issued to another user",
        ));
    }
    if let Some(marker) = store.get_marker(&payload.id)? {
        return duplicate(store, ctx, &payload, &marker, user, now);
    }

    let (accepted, rejected) = validate_all(request, &payload, config);
    let mut by_index: HashMap<u32, &BattleResult> = HashMap::new();
    for battle in &request.battles {
        by_index.entry(battle.index).or_insert(battle);
    }
    let state = user.reputation.state;
    let trusted = is_verifier(&user.reputation, config);
    let held = state == UserState::Quarantined;
    let rate = verify_rate(user.reputation.rep, config);
    let mut random = Random::new(random);

    let mut records = Vec::new();
    let mut seeds = Vec::new();
    let mut checks = Vec::new();
    let mut runs = Vec::new();
    let mut finished = Vec::new();
    let mut given_back = Vec::new();
    let mut slot_battles = Vec::new();
    for &index in &accepted {
        let battle = by_index[&index];
        let assigned = &payload.b[index as usize];
        let ran = matches!(battle.status, BattleStatus::Ok | BattleStatus::RobotFailed);
        let record = || {
            Record::from_battle(
                assigned.division,
                &user.uid,
                &assigned.vids,
                battle,
                trusted,
            )
        };
        let hash = || {
            battle
                .battle_hash
                .clone()
                .filter(|_| battle.status == BattleStatus::Ok)
        };
        match assigned.kind {
            BattleKind::Watch => {}
            BattleKind::Verify => {
                let Some(sort) = &assigned.item else {
                    continue;
                };
                match record() {
                    Ok(record) if ran && state == UserState::New => runs.push((
                        index,
                        sort.clone(),
                        Run {
                            uid: user.uid.clone(),
                            hash: hash(),
                            record: record.to_bytes(),
                            held: false,
                            colors: run_colors(battle),
                        },
                    )),
                    _ => given_back.push(sort.clone()),
                }
            }
            BattleKind::Normal | BattleKind::Probation => {
                if let (BattleKind::Probation, Some(sort)) = (assigned.kind, &assigned.item) {
                    if ran {
                        finished.push(sort.clone());
                    } else {
                        given_back.push(sort.clone());
                    }
                }
                if !ran {
                    continue;
                }
                let Ok(record) = record() else {
                    continue;
                };
                record.encode(&mut records);
                seeds.push(assigned.seed);
                if battle.status != BattleStatus::Ok {
                    continue;
                }
                if held || (state == UserState::New && random.chance(rate)) {
                    checks.push(VerifyItem {
                        engine,
                        sort: format!("{now_ms:013}#{}.{index}", payload.id),
                        division: assigned.division,
                        seed: assigned.seed,
                        bots: assigned.vids.clone(),
                        rounds: assigned.rounds,
                        runs: vec![Run {
                            uid: user.uid.clone(),
                            hash: hash(),
                            record: record.to_bytes(),
                            held,
                            colors: run_colors(battle),
                        }],
                        uids: vec![user.uid.clone()],
                        stage: 1,
                        claimant: None,
                        claim_until: None,
                        kind: VerifyKind::Check,
                        ttl: now + VERIFY_TTL,
                    });
                }
                if !held {
                    slot_battles.push(index);
                }
            }
        }
    }

    let counted = seeds.len() as u64;
    let transaction = ResultsTransaction {
        marker: BatchMarker {
            batch_id: payload.id.clone(),
            uid: user.uid.clone(),
            accepted: accepted.clone(),
            rejected: rejected.clone(),
            slots: Vec::new(),
            ttl: payload.exp + MARKER_TTL,
        },
        log: (!seeds.is_empty()).then(|| LogItem {
            engine,
            hour: hour_key(now),
            sort: format!("{now_ms:013}#{}", payload.id),
            uid: user.uid.clone(),
            records,
            seeds,
            battles: counted as u32,
            held,
            ttl: now + LOG_TTL,
        }),
        uid: user.uid.clone(),
        battles: counted,
        last_post: now,
    };
    match store.commit_results(&transaction) {
        Ok(()) => {}
        Err(StoreError::ConditionFailed) => {
            // Posted concurrently: answer as the repost it is.
            return match store.get_marker(&payload.id)? {
                Some(marker) => duplicate(store, ctx, &payload, &marker, user, now),
                None => Err(ApiError::new(ErrorCode::Unauthorized, "unknown user")),
            };
        }
        Err(e) => return Err(e.into()),
    }

    let mut log = Vec::new();
    for item in &checks {
        ignore_existing(store.put_verify(item))?;
    }
    for sort in &finished {
        verify::finish_probation(store, engine, sort, &user.uid)?;
    }
    for sort in &given_back {
        verify::release_claim(store, engine, sort, &user.uid)?;
    }
    for (index, sort, run) in runs {
        let origin = format!("{}.{index}", payload.id);
        if let Outcome::Disagreement { log: line } =
            verify::resolve(store, config, engine, &sort, run, &origin, now_ms)?
        {
            log.push(line);
        }
    }

    let mut claims: Vec<Claim> = Vec::new();
    if slots::eligible(user, config) {
        for index in slot_battles {
            let assigned = &payload.b[index as usize];
            let division = assigned.division;
            let top = |vid: u32| (ctx.top)(division, vid);
            let pair = |lo: u32, hi: u32| (ctx.slot_pair)(division, lo, hi);
            claims.extend(slots::claim_battle(
                store,
                config,
                engine,
                &payload.id,
                index,
                assigned.seed,
                division,
                &assigned.vids,
                by_index[&index],
                &top,
                &pair,
                now,
            )?);
        }
    }
    if !claims.is_empty() {
        let ids: Vec<String> = claims.iter().map(|c| c.slot.to_string()).collect();
        store.add_marker_slots(&payload.id, &ids)?;
    }
    let mut versions = HashMap::new();
    let mut uploads = Vec::new();
    for claim in &claims {
        if let Some(upload) = presign(store, ctx, &mut versions, &claim.slot, &claim.entry, now)? {
            uploads.push(upload);
        }
    }

    Ok(ResultsOutcome {
        response: ResultsResponse {
            accepted,
            rejected,
            duplicate: false,
            uploads,
            user: summary(user, counted, config),
        },
        log,
    })
}

/// The colours each robot of a battle set, as a verification run keeps them
/// ([`Run::colors`]): malformed ones dropped, empty where none are left.
pub fn run_colors(battle: &BattleResult) -> Vec<Option<BotColors>> {
    let colors: Vec<Option<BotColors>> = battle
        .robots
        .iter()
        .flatten()
        .map(|r| r.colors.as_ref().and_then(BotColors::checked).cloned())
        .collect();
    if colors.iter().all(Option::is_none) {
        Vec::new()
    } else {
        colors
    }
}

fn ignore_existing(result: StoreResult<()>) -> StoreResult<()> {
    match result {
        Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The answer to a repost: the stored outcome and fresh URLs for the slots the batch
/// claimed, hasn't filled and still holds.
fn duplicate<S: Store + ?Sized>(
    store: &S,
    ctx: &ResultsContext,
    payload: &BatchPayload,
    marker: &BatchMarker,
    user: &User,
    now: u64,
) -> Result<ResultsOutcome, ApiError> {
    let engine = ctx.config.engine_version;
    let mut versions = HashMap::new();
    let mut uploads = Vec::new();
    for slot in &marker.slots {
        let Ok(id) = SlotId::parse(slot) else {
            continue;
        };
        let Some(item) = store.get_slots(engine, id.group.division(), &id.group.key())? else {
            continue;
        };
        let Some(entry) = item.entries.get(slot) else {
            continue;
        };
        if entry.batch != payload.id
            || entry.status != SlotStatus::Claimed
            || entry.claim_until <= now
        {
            continue;
        }
        if let Some(upload) = presign(store, ctx, &mut versions, &id, entry, now)? {
            uploads.push(upload);
        }
    }
    Ok(ResultsOutcome {
        response: ResultsResponse {
            accepted: marker.accepted.clone(),
            rejected: marker.rejected.clone(),
            duplicate: true,
            uploads,
            user: summary(user, 0, ctx.config),
        },
        log: Vec::new(),
    })
}

type VersionCache = HashMap<u32, Option<(String, String)>>;

fn presign<S: Store + ?Sized>(
    store: &S,
    ctx: &ResultsContext,
    versions: &mut VersionCache,
    slot: &SlotId,
    entry: &SlotEntry,
    now: u64,
) -> StoreResult<Option<UploadSlot>> {
    let vids = match slot.group {
        slots::Group::Pair { lo, hi } => vec![lo, hi],
        slots::Group::Version(vid) => vec![vid],
    };
    for vid in vids {
        if let std::collections::hash_map::Entry::Vacant(slot) = versions.entry(vid) {
            let found = store
                .get_version(vid)?
                .map(|version| (version.bot, version.sha256));
            slot.insert(found);
        }
    }
    let lookup = |vid: u32| versions.get(&vid).cloned().flatten();
    let Some(key) = slots::recording_key(ctx.config.engine_version, slot, &lookup) else {
        return Ok(None);
    };
    let presigned = ctx
        .presigner
        .put_recording(&key, &entry.sha256_b64, entry.size, now);
    Ok(Some(UploadSlot {
        slot: slot.to_string(),
        index: entry.index,
        round: entry.round,
        url: presigned.url,
        headers: presigned.headers,
        expires: presigned.expires,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ClientInfo, Failure, FailureReason, RobotResult, RoundResult, TrackClaim};
    use crate::ids::WeightClass;
    use crate::store::{MemStore, Reputation, VersionItem, VersionState, rfc3339};

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const NOW_MS: u64 = 1_792_843_200_000;
    const NOW: u64 = NOW_MS / 1000;
    const ROUNDS: u32 = 4;

    fn keys() -> Keys {
        Keys::parse(&format!("a:{KEY}")).unwrap()
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.divisions.one_vs_one.rounds = ROUNDS;
        config
    }

    struct FakePresigner;

    impl Presigner for FakePresigner {
        fn put_recording(&self, key: &str, sha256_b64: &str, size: u64, now: u64) -> Presigned {
            let mut headers = BTreeMap::new();
            headers.insert("Content-Length".to_string(), size.to_string());
            headers.insert("x-amz-checksum-sha256".to_string(), sha256_b64.to_string());
            Presigned {
                url: format!("https://s3.example/{key}?t={now}"),
                headers,
                expires: rfc3339(now + slots::CLAIM_SECONDS),
            }
        }
    }

    fn hex(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn assigned(kind: BattleKind, seed: i64, vids: &[u32], item: Option<&str>) -> BatchBattle {
        BatchBattle {
            division: Division::OneVsOne,
            kind,
            seed,
            rounds: ROUNDS,
            vids: vids.to_vec(),
            item: item.map(str::to_string),
        }
    }

    fn payload(uid: &str, battles: Vec<BatchBattle>) -> BatchPayload {
        BatchPayload {
            v: 1,
            id: "b-1".to_string(),
            uid: uid.to_string(),
            e: 1,
            iat: NOW - 600,
            exp: NOW + 7200,
            b: battles,
            k: String::new(),
        }
    }

    fn robot(score: f64, firsts: u32) -> RobotResult {
        RobotResult {
            score,
            survival: 0.0,
            last_survivor_bonus: 0.0,
            bullet_damage: score,
            bullet_damage_bonus: 0.0,
            ram_damage: 0.0,
            ram_damage_bonus: 0.0,
            firsts,
            seconds: 0,
            thirds: 0,
            rank: 1,
            colors: None,
        }
    }

    /// A valid 1v1 battle whose robot 0 wins rounds 0, 1, 2 and robot 1 round 3.
    fn ok(index: u32, hash: u8) -> BattleResult {
        let winners = [0, 0, 0, 1];
        let rounds: Vec<RoundResult> = winners
            .iter()
            .enumerate()
            .map(|(r, &w)| RoundResult {
                turns: 900,
                winner: w,
                firsts: vec![],
                scores: if w == 0 {
                    vec![100.0, 20.5]
                } else {
                    vec![30.0, 90.0]
                },
                hash: hex(r as u8),
                track: Some(TrackClaim {
                    sha256: hex(100 + r as u8),
                    size: 3000,
                }),
            })
            .collect();
        BattleResult {
            index,
            status: BattleStatus::Ok,
            duration_ms: Some(1000),
            battle_hash: Some(hex(hash)),
            robots: Some(vec![robot(330.0, 3), robot(151.5, 1)]),
            rounds: Some(rounds),
            failure: None,
        }
    }

    fn status(index: u32, status: BattleStatus) -> BattleResult {
        BattleResult {
            index,
            status,
            duration_ms: None,
            battle_hash: None,
            robots: None,
            rounds: None,
            failure: (status == BattleStatus::RobotFailed).then_some(Failure {
                robot: 1,
                reason: FailureReason::Crash,
                detail: None,
            }),
        }
    }

    fn request(payload: &BatchPayload, battles: Vec<BattleResult>) -> ResultsRequest {
        ResultsRequest {
            batch: token::sign(payload, &keys()).unwrap(),
            engine_version: 1,
            client: ClientInfo {
                release: "0.2.0".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                sandbox: "os".into(),
            },
            battles,
        }
    }

    fn setup(rep: u32, state: UserState) -> (MemStore, User) {
        let store = MemStore::new();
        let user = User {
            uid: "g1".to_string(),
            battles: 10,
            reputation: Reputation {
                rep,
                state,
                ..Reputation::default()
            },
            ..User::default()
        };
        store.create_user(&user).unwrap();
        for (vid, bot) in [(5, "voidious.Diamond"), (9, "jk.mega.DrussGT")] {
            store
                .put_version(&VersionItem {
                    vid,
                    bot: bot.to_string(),
                    version: "1".into(),
                    sha256: hex(vid as u8),
                    size: 1,
                    codesize: None,
                    class: WeightClass::Mega,
                    divisions: vec![Division::OneVsOne],
                    state: VersionState::Active,
                    probation_battles: 0,
                    probation_failures: 0,
                    class_name: bot.to_string(),
                    created: 0,
                    retired: None,
                    url: None,
                    colors: None,
                })
                .unwrap();
        }
        (store, user)
    }

    fn post_with(
        store: &MemStore,
        user: &User,
        config: &Config,
        top: &dyn Fn(Division, u32) -> bool,
        request: &ResultsRequest,
        now_ms: u64,
    ) -> Result<ResultsOutcome, ApiError> {
        let keys = keys();
        let ctx = ResultsContext {
            config,
            keys: &keys,
            top,
            slot_pair: &|_, _, _| false,
            presigner: &FakePresigner,
        };
        apply_results(store, &ctx, user, request, now_ms, &[9; 32])
    }

    fn post(
        store: &MemStore,
        user: &User,
        request: &ResultsRequest,
    ) -> Result<ResultsOutcome, ApiError> {
        post_with(store, user, &config(), &|_, _| false, request, NOW_MS)
    }

    fn check(battle: BattleResult) -> Result<(), String> {
        validate(
            &battle,
            &assigned(BattleKind::Normal, 1, &[5, 9], None),
            &config(),
        )
    }

    #[test]
    fn validation_rules() {
        assert_eq!(check(ok(0, 1)), Ok(()));
        assert_eq!(check(status(0, BattleStatus::NotRun)), Ok(()));
        assert_eq!(check(status(0, BattleStatus::Error)), Ok(()));
        assert_eq!(check(status(0, BattleStatus::RobotFailed)), Ok(()));

        let broken = |edit: &dyn Fn(&mut BattleResult), expected: &str| {
            let mut battle = ok(0, 1);
            edit(&mut battle);
            let message = check(battle).expect_err(expected);
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        };
        fn robots(b: &mut BattleResult) -> &mut Vec<RobotResult> {
            b.robots.as_mut().unwrap()
        }
        fn rounds(b: &mut BattleResult) -> &mut Vec<RoundResult> {
            b.rounds.as_mut().unwrap()
        }
        // robots.len() = the battle's bots
        broken(&|b| robots(b).truncate(1), "robots: 1 of 2");
        broken(&|b| b.robots = None, "robots missing");
        // rounds.len() = its rounds
        broken(&|b| rounds(b).truncate(3), "rounds: 3 of 4");
        broken(&|b| b.rounds = None, "rounds missing");
        // finite non-negative scores
        broken(&|b| robots(b)[0].score = f64::NAN, "robots[0].score");
        broken(
            &|b| robots(b)[1].bullet_damage = -1.0,
            "robots[1].bulletDamage",
        );
        broken(
            &|b| rounds(b)[0].scores[0] = f64::INFINITY,
            "rounds[0].scores[0]",
        );
        broken(&|b| rounds(b)[2].scores[1] = -0.5, "rounds[2].scores[1]");
        broken(
            &|b| rounds(b)[1].scores.pop().map(|_| ()).unwrap(),
            "rounds[1].scores: 1 of 2",
        );
        // the round scores sum to the score within 1e-6 relative
        broken(&|b| robots(b)[0].score = 330.001, "rounds sum to 330");
        let mut close = ok(0, 1);
        robots(&mut close)[0].score = 330.0 * (1.0 + 5e-7);
        assert_eq!(check(close), Ok(()));
        // firsts = rounds won
        broken(
            &|b| robots(b)[1].firsts = 2,
            "robots[1].firsts: 2 but it won 1",
        );
        broken(&|b| rounds(b)[0].winner = 2, "rounds[0].winner");
        broken(&|b| rounds(b)[0].winner = -2, "rounds[0].winner");
        let mut nobody = ok(0, 1);
        rounds(&mut nobody)[0].winner = -1;
        robots(&mut nobody)[0].firsts = 2;
        assert_eq!(check(nobody), Ok(()));
        // A tie: robots that die together each get a first (classic); winner -1.
        let tie = |b: &mut BattleResult| {
            rounds(b)[0].firsts = vec![0, 1];
            rounds(b)[0].winner = -1;
            robots(b)[1].firsts = 2;
        };
        let mut tied = ok(0, 1);
        tie(&mut tied);
        assert_eq!(check(tied), Ok(()));
        broken(
            &|b| {
                tie(b);
                rounds(b)[0].winner = 0;
            },
            "rounds[0].winner: 0 with firsts",
        );
        broken(
            &|b| {
                tie(b);
                rounds(b)[0].firsts = vec![0, 0];
            },
            "rounds[0].firsts",
        );
        broken(&|b| rounds(b)[0].firsts = vec![2], "rounds[0].firsts");
        // hashes of 64 lowercase hex digits
        broken(&|b| b.battle_hash = None, "battleHash missing");
        broken(
            &|b| b.battle_hash = Some(hex(0xab).to_uppercase()),
            "battleHash",
        );
        broken(&|b| rounds(b)[3].hash = "abc".into(), "rounds[3].hash");
        broken(
            &|b| rounds(b)[3].track.as_mut().unwrap().sha256 = "x".into(),
            "rounds[3].track.sha256",
        );
        // track.size 1..slots.maxBytes
        broken(
            &|b| rounds(b)[0].track.as_mut().unwrap().size = 0,
            "track.size",
        );
        broken(
            &|b| rounds(b)[0].track.as_mut().unwrap().size = 262_145,
            "track.size",
        );
        let mut biggest = ok(0, 1);
        rounds(&mut biggest)[0].track.as_mut().unwrap().size = 262_144;
        assert_eq!(check(biggest), Ok(()));
        // robotFailed names a robot of the battle
        let mut failed = status(0, BattleStatus::RobotFailed);
        failed.failure.as_mut().unwrap().robot = 2;
        assert!(check(failed).unwrap_err().contains("failure.robot"));
        let mut failed = status(0, BattleStatus::RobotFailed);
        failed.failure = None;
        assert!(check(failed).is_err());
    }

    #[test]
    fn indices_are_in_the_batch_and_once() {
        let p = payload(
            "g1",
            vec![assigned(BattleKind::Normal, 1, &[5, 9], None); 2],
        );
        let r = request(&p, vec![ok(1, 1), ok(0, 2), ok(1, 3), ok(2, 4)]);
        let (accepted, rejected) = validate_all(&r, &p, &config());
        assert_eq!(accepted, [0, 1]);
        let messages: Vec<(u32, &str)> = rejected
            .iter()
            .map(|r| (r.index, r.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [(1, "index 1 twice"), (2, "index 2 is not in the batch")]
        );
        assert!(rejected.iter().all(|r| r.error == "invalid"));
    }

    #[test]
    fn a_post_counts_once() {
        let (store, user) = setup(60, UserState::New);
        let p = payload(
            "g1",
            vec![
                assigned(BattleKind::Normal, 11, &[5, 9], None),
                assigned(BattleKind::Normal, 12, &[9, 5], None),
                assigned(BattleKind::Normal, 13, &[5, 9], None),
                assigned(BattleKind::Normal, 14, &[5, 9], None),
            ],
        );
        let mut bad = ok(2, 3);
        bad.robots.as_mut().unwrap()[0].firsts = 0;
        let r = request(
            &p,
            vec![
                ok(0, 1),
                status(1, BattleStatus::RobotFailed),
                bad,
                status(3, BattleStatus::NotRun),
            ],
        );
        let first = post(&store, &user, &r).unwrap();
        assert_eq!(first.response.accepted, [0, 1, 3]);
        assert_eq!(first.response.rejected.len(), 1);
        assert_eq!(first.response.rejected[0].index, 2);
        assert!(!first.response.duplicate);
        assert_eq!(first.response.user.battles, 12);
        assert_eq!(first.response.user.tier, crate::api::Tier::Verifier);

        let logs = store.log_items(1);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].hour, 2_026_102_412);
        assert_eq!(logs[0].sort, format!("{NOW_MS}#b-1"));
        assert_eq!(logs[0].seeds, [11, 12]);
        assert!(!logs[0].held);
        let records = Record::decode_all(&logs[0].records, "g1").unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.trusted));
        assert_eq!(records[0].contestants[0].score, 330.0);
        assert!(records[1].failed.is_some());
        assert_eq!(store.get_user("g1").unwrap().unwrap().battles, 12);

        // The repost: same answer, nothing counted again.
        let again = post(&store, &user, &r).unwrap();
        assert!(again.response.duplicate);
        assert_eq!(again.response.accepted, first.response.accepted);
        assert_eq!(again.response.rejected, first.response.rejected);
        assert_eq!(store.log_items(1).len(), 1);
        assert_eq!(store.get_user("g1").unwrap().unwrap().battles, 12);
    }

    #[test]
    fn token_engine_and_deadline_checks() {
        let (store, user) = setup(60, UserState::New);
        let p = payload("g1", vec![assigned(BattleKind::Normal, 1, &[5, 9], None)]);
        let r = request(&p, vec![ok(0, 1)]);
        // Past the deadline.
        let late = post_with(&store, &user, &config(), &|_, _| false, &r, (p.exp) * 1000);
        assert_eq!(late.unwrap_err().code, ErrorCode::Expired);
        // The request's engine.
        let mut old = r.clone();
        old.engine_version = 0;
        assert_eq!(
            post(&store, &user, &old).unwrap_err().code,
            ErrorCode::EngineMismatch
        );
        // The token's engine.
        let mut other = p.clone();
        other.e = 2;
        let r2 = request(&other, vec![ok(0, 1)]);
        assert_eq!(
            post(&store, &user, &r2).unwrap_err().code,
            ErrorCode::EngineMismatch
        );
        // Another user's batch, a forged token.
        let theirs = request(&payload("g2", p.b.clone()), vec![ok(0, 1)]);
        assert_eq!(
            post(&store, &user, &theirs).unwrap_err().code,
            ErrorCode::Forbidden
        );
        let mut forged = r.clone();
        forged.batch.push('x');
        assert_eq!(
            post(&store, &user, &forged).unwrap_err().code,
            ErrorCode::BadRequest
        );
        // Banned.
        let mut banned = user.clone();
        banned.reputation.state = UserState::Banned;
        assert_eq!(
            post(&store, &banned, &r).unwrap_err().code,
            ErrorCode::Forbidden
        );
        assert!(store.get_marker("b-1").unwrap().is_none());
    }

    #[test]
    fn picking_battles_for_verification() {
        let battles = vec![assigned(BattleKind::Normal, 1, &[5, 9], None); 3];
        // Robot 1 of battle 1 reports its colours, robot 0 malformed ones (dropped).
        let mut colored = ok(1, 2);
        let robots = colored.robots.as_mut().unwrap();
        robots[0].colors = Some(BotColors {
            body: Some("red".into()),
            ..Default::default()
        });
        robots[1].colors = Some(BotColors {
            gun: Some("#ff0000".into()),
            ..Default::default()
        });
        let r = |p: &BatchPayload| {
            request(
                p,
                vec![
                    ok(0, 1),
                    colored.clone(),
                    status(2, BattleStatus::RobotFailed),
                ],
            )
        };
        // Rate 1: every ok battle.
        let (store, user) = setup(0, UserState::New);
        let mut always = config();
        always.rep.start_rate = 1.0;
        always.rep.floor_rate = 1.0;
        let p = payload("g1", battles.clone());
        post_with(&store, &user, &always, &|_, _| false, &r(&p), NOW_MS).unwrap();
        let items = store.verify_items(1);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].sort, format!("{NOW_MS}#b-1.0"));
        assert_eq!(items[0].runs[0].hash, Some(hex(1)));
        assert!(!items[0].runs[0].held);
        assert_eq!(items[0].kind, VerifyKind::Check);
        // The run keeps the colours reported, for a second run to confirm.
        assert!(items[0].runs[0].colors.is_empty());
        assert_eq!(
            items[1].runs[0].colors,
            [
                None,
                Some(BotColors {
                    gun: Some("#ff0000".into()),
                    ..Default::default()
                })
            ]
        );
        // Rate 0: none.
        let (store, user) = setup(0, UserState::New);
        let mut never = config();
        never.rep.start_rate = 0.0;
        never.rep.floor_rate = 0.0;
        post_with(&store, &user, &never, &|_, _| false, &r(&p), NOW_MS).unwrap();
        assert!(store.verify_items(1).is_empty());
        // The rate follows the random bytes: about 20% of many draws at rep 0.
        let mut random = Random::new(&[1; 32]);
        let hits = (0..10_000).filter(|_| random.chance(0.2)).count();
        assert!((1_800..2_200).contains(&hits), "{hits}");
    }

    #[test]
    fn quarantined_users_are_held_and_all_checked() {
        let (store, user) = setup(0, UserState::Quarantined);
        let p = payload(
            "g1",
            vec![assigned(BattleKind::Normal, 1, &[5, 9], None); 2],
        );
        let r = request(&p, vec![ok(0, 1), ok(1, 2)]);
        let top = |_: Division, _: u32| true;
        let out = post_with(&store, &user, &config(), &top, &r, NOW_MS).unwrap();
        assert!(
            out.response.uploads.is_empty(),
            "no recordings from quarantined users"
        );
        assert_eq!(out.response.user.tier, crate::api::Tier::Quarantined);
        let logs = store.log_items(1);
        assert!(logs[0].held);
        let items = store.verify_items(1);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.runs[0].held));
        assert!(!Record::decode_all(&logs[0].records, "g1").unwrap()[0].trusted);
    }

    #[test]
    fn verification_and_probation_work_comes_back() {
        let (store, user) = setup(60, UserState::New);
        store
            .create_user(&User {
                uid: "g2".into(),
                ..User::default()
            })
            .unwrap();
        // A check item of g2's battle, and a probation item, both claimed by g1.
        let theirs = ok(0, 7);
        let record =
            Record::from_battle(Division::OneVsOne, "g2", &[5, 9], &theirs, false).unwrap();
        for (sort, kind, runs) in [
            (
                "0000000000001#x.0",
                VerifyKind::Check,
                vec![Run {
                    uid: "g2".into(),
                    hash: Some(hex(7)),
                    record: record.to_bytes(),
                    held: true,
                    colors: Vec::new(),
                }],
            ),
            ("0000000000002#p", VerifyKind::Probation, vec![]),
            (
                "0000000000003#x.1",
                VerifyKind::Check,
                vec![Run {
                    uid: "g2".into(),
                    hash: Some(hex(8)),
                    record: record.to_bytes(),
                    held: false,
                    colors: Vec::new(),
                }],
            ),
        ] {
            store
                .put_verify(&VerifyItem {
                    engine: 1,
                    sort: sort.into(),
                    division: Division::OneVsOne,
                    seed: 5,
                    bots: vec![5, 9],
                    rounds: ROUNDS,
                    uids: runs.iter().map(|r| r.uid.clone()).collect(),
                    runs,
                    stage: 1,
                    claimant: None,
                    claim_until: None,
                    kind,
                    ttl: 0,
                })
                .unwrap();
        }
        let taken = verify::take_work(&store, 1, "g1", 5, NOW + 3600, NOW).unwrap();
        assert_eq!(taken.len(), 3);
        let p = payload(
            "g1",
            vec![
                assigned(BattleKind::Verify, 5, &[5, 9], Some("0000000000001#x.0")),
                assigned(BattleKind::Probation, 6, &[5, 9], Some("0000000000002#p")),
                assigned(BattleKind::Verify, 5, &[5, 9], Some("0000000000003#x.1")),
            ],
        );
        let r = request(
            &p,
            vec![ok(0, 7), ok(1, 1), status(2, BattleStatus::NotRun)],
        );
        let mut never = config();
        never.rep.start_rate = 0.0;
        never.rep.floor_rate = 0.0;
        post_with(&store, &user, &never, &|_, _| false, &r, NOW_MS).unwrap();
        // The check matched: item gone, both credited, g2's held record released.
        let left = store.verify_items(1);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].sort, "0000000000003#x.1");
        assert!(
            left[0].claimant.is_none(),
            "the unrun one is back in the queue"
        );
        assert_eq!(store.get_user("g1").unwrap().unwrap().reputation.rep, 61);
        assert_eq!(store.get_user("g2").unwrap().unwrap().reputation.rep, 1);
        let logs = store.log_items(1);
        let uids: Vec<&str> = logs.iter().map(|l| l.uid.as_str()).collect();
        assert_eq!(uids, ["g1", "g2"]);
        // g1's log item holds the probation battle only: the verify run isn't counted again.
        assert_eq!(logs[0].seeds, [6]);
        assert_eq!(logs[1].seeds, [5]);
    }

    #[test]
    fn slots_are_claimed_and_offered_again_on_repost() {
        let (store, user) = setup(30, UserState::New);
        let mut config = config();
        config.slots.per_side = 2;
        let top = |_: Division, vid: u32| vid == 9;
        let p = payload(
            "g1",
            vec![assigned(BattleKind::Normal, 11, &[5, 9], None); 2],
        );
        let r = request(&p, vec![ok(0, 1), ok(1, 2)]);
        let out = post_with(&store, &user, &config, &top, &r, NOW_MS).unwrap();
        let uploads = &out.response.uploads;
        let slots: Vec<(&str, u32, u32)> = uploads
            .iter()
            .map(|u| (u.slot.as_str(), u.index, u.round))
            .collect();
        // Robot 0 (vid 5, the lower) wins rounds 0..2: a.0, a.1 from battle 0, b.0 from its
        // round 3, b.1 from battle 1's round 3.
        assert_eq!(
            slots,
            [
                ("1v1.5.9.a.0", 0, 0),
                ("1v1.5.9.a.1", 0, 1),
                ("1v1.5.9.b.0", 0, 3),
                ("1v1.5.9.b.1", 1, 3)
            ]
        );
        // jk.mega.DrussGT (vid 9) sorts first, so vid 5 winning is `b` in the key.
        assert_eq!(
            uploads[0].url,
            format!(
                "https://s3.example/rec/e1/1v1/jk.mega.DrussGT_{}/voidious.Diamond_{}/b-0.track?t={NOW}",
                &hex(9)[..16],
                &hex(5)[..16]
            )
        );
        assert_eq!(uploads[0].headers["Content-Length"], "3000");
        assert_eq!(store.get_marker("b-1").unwrap().unwrap().slots.len(), 4);

        // One is filled; the repost offers the other three again.
        let filled = &uploads[1];
        let sha = filled.headers["x-amz-checksum-sha256"].clone();
        assert_eq!(
            slots::confirm(&store, &config, 1, "b-1", &filled.slot, Some(&sha)).unwrap(),
            Ok(())
        );
        let again = post_with(&store, &user, &config, &top, &r, NOW_MS + 60_000).unwrap();
        assert!(again.response.duplicate);
        let offered: Vec<&str> = again
            .response
            .uploads
            .iter()
            .map(|u| u.slot.as_str())
            .collect();
        assert_eq!(offered, ["1v1.5.9.a.0", "1v1.5.9.b.0", "1v1.5.9.b.1"]);
        assert!(
            again.response.uploads[0]
                .url
                .ends_with(&format!("?t={}", NOW + 60))
        );
        // After the claims expire, nothing is offered.
        let expired = post_with(&store, &user, &config, &top, &r, NOW_MS + 3_600_000).unwrap();
        assert!(expired.response.uploads.is_empty());
    }

    #[test]
    fn two_batches_race_for_the_same_slots() {
        let (store, user) = setup(30, UserState::New);
        let mut config = config();
        config.slots.per_side = 3;
        let top = |_: Division, _: u32| true;
        let mut first = payload("g1", vec![assigned(BattleKind::Normal, 1, &[5, 9], None)]);
        first.id = "b-A".into();
        let mut second = first.clone();
        second.id = "b-B".into();
        let a = post_with(
            &store,
            &user,
            &config,
            &top,
            &request(&first, vec![ok(0, 1)]),
            NOW_MS,
        )
        .unwrap();
        let b = post_with(
            &store,
            &user,
            &config,
            &top,
            &request(&second, vec![ok(0, 1)]),
            NOW_MS,
        )
        .unwrap();
        let ids = |o: &ResultsOutcome| -> Vec<String> {
            o.response.uploads.iter().map(|u| u.slot.clone()).collect()
        };
        // Three a-rounds and one b-round per battle: the first takes a.0..a.2 and b.0, the
        // second only what is left on the b side.
        assert_eq!(
            ids(&a),
            ["1v1.5.9.a.0", "1v1.5.9.a.1", "1v1.5.9.a.2", "1v1.5.9.b.0"]
        );
        assert_eq!(ids(&b), ["1v1.5.9.b.1"]);
    }
}
