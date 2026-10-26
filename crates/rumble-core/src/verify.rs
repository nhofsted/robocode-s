// SPDX-License-Identifier: EPL-2.0

//! Verification and reputation (spec §11): the verification rate and tiers, serving the
//! verification queue to verifiers, and the state machine that resolves a verifier's run:
//! stage 1 (match or mismatch), stage 2 (majority of three), penalties (quarantine, ban),
//! releases of held records, correction records and rechecks.

use crate::api::Tier;
use crate::config::Config;
use crate::ids::Division;
use crate::record::Record;
use crate::store::{
    ALERT_TTL, Alert, DAY, LOG_TTL, LogItem, Reputation, Run, SYSTEM_UID, Store, StoreError,
    StoreResult, UserState, VERIFY_TTL, VerifyItem, VerifyKind, hour_key,
};

/// The `kind` of the alert a ban leaves.
pub const BAN_ALERT: &str = "ban";

/// The alert of a ban (§11.3): the `cleanup` job finds banned users by these alerts (the
/// table has no index of users) and writes the correction records. `reference` is the uid.
pub fn ban_alert(uid: &str, why: &str, now_ms: u64) -> Alert {
    Alert {
        sort: format!("{now_ms:013}#ban.{uid}"),
        kind: BAN_ALERT.to_string(),
        text: format!("{uid} banned: {why}"),
        reference: uid.to_string(),
        ttl: now_ms / 1000 + ALERT_TTL,
    }
}

/// Items read per queue query when serving work (§10.4).
pub const QUEUE_PAGE: usize = 25;
/// Queue pages read at most per request for work: past the items a verifier can't take
/// (its own runs, claimed items, other divisions), which can fill the oldest pages.
pub const QUEUE_PAGES: usize = 40;
/// Hours of the result log searched for a penalized user's recent battles (§11.3).
pub const RECHECK_HOURS: u64 = 48;
/// Tries of a reputation read-modify-write before giving up.
const UPDATE_TRIES: usize = 8;

/// The verification rate of a user with reputation `rep` (§11.1):
/// `max(floorRate, startRate · e^(−rep/tau))`.
pub fn verify_rate(rep: u32, config: &Config) -> f64 {
    let r = &config.rep;
    (r.start_rate * (-f64::from(rep) / r.tau).exp()).max(r.floor_rate)
}

/// The tier shown for a user (§11.1).
pub fn tier(reputation: &Reputation, config: &Config) -> Tier {
    match reputation.state {
        UserState::Banned => Tier::Banned,
        UserState::Quarantined => Tier::Quarantined,
        UserState::New if reputation.rep >= config.rep.verifier_min => Tier::Verifier,
        UserState::New => Tier::New,
    }
}

/// Whether a user may get verification, probation and watch work (§10.4): state `new` and
/// reputation at least `rep.verifierMin`. The same test sets a record's `trusted` flag.
pub fn is_verifier(reputation: &Reputation, config: &Config) -> bool {
    reputation.state == UserState::New && reputation.rep >= config.rep.verifier_min
}

/// The id of a failure reporter (§12.3), kept by the planner in the pairing state.
pub use crate::state::reporter_hash as reporter_id;

/// A match credited to a user (§11.3): `rep += 1, vm += 1`; a quarantined user gets
/// `qm += 1` instead of `rep` and returns to `new` with `rep` 0 at `rep.quarantineMatches`.
/// Banned users get nothing.
pub fn credit(reputation: &mut Reputation, config: &Config) {
    match reputation.state {
        UserState::Banned => {}
        UserState::New => {
            reputation.rep += 1;
            reputation.verified_matches += 1;
        }
        UserState::Quarantined => {
            reputation.verified_matches += 1;
            reputation.quarantine_matches += 1;
            if reputation.quarantine_matches >= config.rep.quarantine_matches {
                reputation.state = UserState::New;
                reputation.rep = 0;
                reputation.quarantine_matches = 0;
            }
        }
    }
}

/// A confirmed mismatch (§11.3): `rep = 0`, quarantined, `mm += 1`, the time appended to
/// `mmt` (pruned to the ban window), and banned at `rep.banMismatches` mismatches within
/// `rep.banWindowDays`. Returns whether the user is banned now.
pub fn penalize(reputation: &mut Reputation, config: &Config, now: u64) -> bool {
    let window = u64::from(config.rep.ban_window_days) * DAY;
    reputation.rep = 0;
    reputation.quarantine_matches = 0;
    reputation.mismatches += 1;
    reputation
        .mismatch_times
        .retain(|&time| time.saturating_add(window) > now);
    reputation.mismatch_times.push(now);
    if reputation.state != UserState::Banned {
        reputation.state = if reputation.mismatch_times.len() >= config.rep.ban_mismatches as usize
        {
            UserState::Banned
        } else {
            UserState::Quarantined
        };
    }
    reputation.state == UserState::Banned
}

/// Read-modify-write of a user's reputation, retried while concurrent changes intervene.
/// Returns the new reputation, or `None` if there is no such user.
pub fn update_reputation<S: Store + ?Sized>(
    store: &S,
    uid: &str,
    change: impl Fn(&mut Reputation),
) -> StoreResult<Option<Reputation>> {
    for _ in 0..UPDATE_TRIES {
        let Some(user) = store.get_user(uid)? else {
            return Ok(None);
        };
        let mut reputation = user.reputation.clone();
        change(&mut reputation);
        match store.set_reputation(uid, user.reputation.ver, &reputation) {
            Ok(()) => {
                reputation.ver = user.reputation.ver + 1;
                return Ok(Some(reputation));
            }
            Err(StoreError::ConditionFailed) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Busy)
}

/// Whether two runs agree: equal battle hashes, or, when one of them has no hash (a
/// recheck's original run, a `robotFailed` run), equal records (division, contestants with
/// exact scores and firsts, failure). The reporter's `trusted` flag and weight don't matter.
pub fn runs_agree(a: &Run, b: &Run) -> bool {
    match (&a.hash, &b.hash) {
        (Some(x), Some(y)) => x == y,
        _ => match (Record::decode(&a.record, ""), Record::decode(&b.record, "")) {
            (Ok((x, _)), Ok((y, _))) => {
                x.division == y.division && x.contestants == y.contestants && x.failed == y.failed
            }
            _ => false,
        },
    }
}

/// Serves up to `max` verification items to a verifier (§10.4 step 2): query pages, oldest
/// first, up to [`QUEUE_PAGES`], skipping items it ran or that are claimed, each claimed with a conditional
/// update (`cl = uid, cu = until`). Items lost to a concurrent claim are skipped.
pub fn take_work<S: Store + ?Sized>(
    store: &S,
    engine: u32,
    uid: &str,
    max: usize,
    until: u64,
    now: u64,
) -> StoreResult<Vec<VerifyItem>> {
    take_work_in(store, engine, uid, None, max, until, now)
}

/// [`take_work`] restricted to one division (a batch is one division, §10.4): items of
/// other divisions in the page are skipped.
pub fn take_work_in<S: Store + ?Sized>(
    store: &S,
    engine: u32,
    uid: &str,
    division: Option<Division>,
    max: usize,
    until: u64,
    now: u64,
) -> StoreResult<Vec<VerifyItem>> {
    let mut taken = Vec::new();
    if max == 0 {
        return Ok(taken);
    }
    let mut after: Option<String> = None;
    for _ in 0..QUEUE_PAGES {
        let page = store.query_verify(engine, after.as_deref(), QUEUE_PAGE)?;
        let last = page.len() < QUEUE_PAGE;
        after = page.last().map(|item| item.sort.clone());
        for mut item in page {
            if taken.len() >= max {
                return Ok(taken);
            }
            if division.is_some_and(|d| d != item.division)
                || item.uids.iter().any(|u| u == uid)
                || !item.claimable(now)
            {
                continue;
            }
            match store.claim_verify(engine, &item.sort, uid, until, now) {
                Ok(()) => {
                    item.claimant = Some(uid.to_string());
                    item.claim_until = Some(until);
                    taken.push(item);
                }
                Err(StoreError::ConditionFailed) => continue,
                Err(e) => return Err(e),
            }
        }
        if last || taken.len() >= max {
            break;
        }
    }
    Ok(taken)
}

/// Gives an item back to the queue (the verifier didn't run it): clears the claim if it is
/// still `uid`'s.
pub fn release_claim<S: Store + ?Sized>(
    store: &S,
    engine: u32,
    sort: &str,
    uid: &str,
) -> StoreResult<()> {
    let Some(mut item) = store.get_verify(engine, sort)? else {
        return Ok(());
    };
    if item.claimant.as_deref() != Some(uid) {
        return Ok(());
    }
    item.claimant = None;
    item.claim_until = None;
    match store.replace_verify(&item, uid) {
        Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Ends a probation item whose battle came back (ok or failed): it counted as a normal
/// battle in the poster's log item. Only the claimant can end it.
pub fn finish_probation<S: Store + ?Sized>(
    store: &S,
    engine: u32,
    sort: &str,
    uid: &str,
) -> StoreResult<()> {
    match store.delete_verify(engine, sort, Some(uid)) {
        Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
        Err(e) => Err(e),
    }
}

/// What resolving a verifier's run did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing: the item is gone, isn't a check or recheck, or isn't claimed by this
    /// verifier (its claim expired and someone else took it).
    Ignored,
    /// Stage 1 agreed: both credited, a held record released, the item deleted.
    Matched,
    /// Stage 1 disagreed: the item waits for a third run.
    TieBreak,
    /// Stage 2: two runs agreed. `penalized` is the third run's user (`None` for a
    /// recheck's original run, which is corrected but not penalized again); `corrected`
    /// tells whether correction records were written; `banned` whether the penalty banned.
    Majority {
        minority: String,
        penalized: Option<String>,
        corrected: bool,
        banned: bool,
    },
    /// Stage 2: all three differ. An alert was stored; `log` is the line the Lambda must log
    /// (a CloudWatch metric filter turns it into an alarm). Nobody is penalized.
    Disagreement { log: String },
}

/// Resolves a verifier's run of the verification item `sort` (§11.3). `run.uid` is the
/// verifier, `run.hash` its battle hash and `run.record` its record. `origin` makes the
/// sort keys of the items this writes unique and idempotent (the Lambda passes
/// `<batchId>.<index>`). `now_ms` is the current time in milliseconds.
pub fn resolve<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    engine: u32,
    sort: &str,
    run: Run,
    origin: &str,
    now_ms: u64,
) -> StoreResult<Outcome> {
    let Some(mut item) = store.get_verify(engine, sort)? else {
        return Ok(Outcome::Ignored);
    };
    let uid = run.uid.clone();
    if item.claimant.as_deref() != Some(uid.as_str())
        || item.uids.contains(&uid)
        || item.kind == VerifyKind::Probation
        || item.runs.is_empty()
        || item.runs.len() > 2
    {
        return Ok(Outcome::Ignored);
    }
    if item.runs.len() == 1 {
        let original = item.runs[0].clone();
        if runs_agree(&original, &run) {
            if !delete_claimed(store, engine, sort, &uid)? {
                return Ok(Outcome::Ignored);
            }
            confirm_colors(store, &item.bots, &original, &run)?;
            credit_user(store, config, &uid)?;
            credit_user(store, config, &original.uid)?;
            if original.held {
                release(store, &item, &original, origin, now_ms)?;
            }
            return Ok(Outcome::Matched);
        }
        item.uids.push(uid.clone());
        item.runs.push(run);
        item.stage = 2;
        item.claimant = None;
        item.claim_until = None;
        return match store.replace_verify(&item, &uid) {
            Ok(()) => Ok(Outcome::TieBreak),
            Err(StoreError::ConditionFailed) => Ok(Outcome::Ignored),
            Err(e) => Err(e),
        };
    }

    // Stage 2: the original, the first verifier and this run.
    let original = item.runs[0].clone();
    let first = item.runs[1].clone();
    let with_original = runs_agree(&original, &run);
    let with_first = runs_agree(&first, &run);
    if !delete_claimed(store, engine, sort, &uid)? {
        return Ok(Outcome::Ignored);
    }
    let now = now_ms / 1000;
    match (with_original, with_first) {
        (true, false) => {
            // The first verifier was wrong; the original stands.
            confirm_colors(store, &item.bots, &original, &run)?;
            credit_user(store, config, &original.uid)?;
            credit_user(store, config, &uid)?;
            if original.held {
                release(store, &item, &original, origin, now_ms)?;
            }
            let banned = penalize_user(store, config, engine, &first.uid, None, origin, now_ms)?;
            Ok(Outcome::Majority {
                penalized: Some(first.uid.clone()),
                minority: first.uid,
                corrected: false,
                banned,
            })
        }
        (false, true) => {
            // The original was wrong. If it was counted, correct it.
            confirm_colors(store, &item.bots, &first, &run)?;
            credit_user(store, config, &first.uid)?;
            credit_user(store, config, &uid)?;
            let corrected = !original.held;
            if corrected {
                correct(store, &item, &original, &run, origin, now_ms)?;
            }
            let (penalized, banned) = if item.kind == VerifyKind::Recheck {
                // Already penalized for the mismatch that caused the recheck.
                (None, false)
            } else {
                let banned = penalize_user(
                    store,
                    config,
                    engine,
                    &original.uid,
                    Some(item.seed),
                    origin,
                    now_ms,
                )?;
                (Some(original.uid.clone()), banned)
            };
            Ok(Outcome::Majority {
                minority: original.uid,
                penalized,
                corrected,
                banned,
            })
        }
        _ => {
            let describe = |r: &Run| format!("{}={}", r.uid, r.hash.as_deref().unwrap_or("-"));
            let alert = Alert {
                sort: format!("{now_ms:013}#{origin}"),
                kind: "engine-mismatch".to_string(),
                text: format!(
                    "three runs of verification item {sort} disagree: {}, {}, {}",
                    describe(&original),
                    describe(&first),
                    describe(&run)
                ),
                reference: sort.to_string(),
                ttl: now + ALERT_TTL,
            };
            store.put_alert(&alert)?;
            Ok(Outcome::Disagreement {
                log: format!("ALERT engine-mismatch {sort}"),
            })
        }
    }
}

/// Keeps the colours two agreeing runs of a battle report alike for a robot as its version's
/// ([`Store::set_version_colors`]), unless the version has colours already: the first
/// confirmed colours stay, also for robots whose colours change from battle to battle. Two
/// users reporting the same colours for the same battle (the battle hash covers the colour
/// values, through every snapshot's check) is what makes them trusted; a single post never
/// sets them. `bots` is the battle's robot order.
fn confirm_colors<S: Store + ?Sized>(store: &S, bots: &[u32], a: &Run, b: &Run) -> StoreResult<()> {
    for ((vid, x), y) in bots.iter().zip(&a.colors).zip(&b.colors) {
        if let (Some(x), Some(y)) = (x, y)
            && x == y
        {
            ignore_existing(store.set_version_colors(*vid, x))?;
        }
    }
    Ok(())
}

/// Deletes the item if it is still claimed by `uid`; `false` if it isn't.
fn delete_claimed<S: Store + ?Sized>(
    store: &S,
    engine: u32,
    sort: &str,
    uid: &str,
) -> StoreResult<bool> {
    match store.delete_verify(engine, sort, Some(uid)) {
        Ok(()) => Ok(true),
        Err(StoreError::ConditionFailed) => Ok(false),
        Err(e) => Err(e),
    }
}

fn credit_user<S: Store + ?Sized>(store: &S, config: &Config, uid: &str) -> StoreResult<()> {
    if uid != SYSTEM_UID {
        update_reputation(store, uid, |r| credit(r, config))?;
    }
    Ok(())
}

/// Releases a held record: a log item of its own with the original's uid (the verifier's
/// log item can't carry it: a log item has one uid).
fn release<S: Store + ?Sized>(
    store: &S,
    item: &VerifyItem,
    original: &Run,
    origin: &str,
    now_ms: u64,
) -> StoreResult<()> {
    let log = LogItem {
        engine: item.engine,
        hour: hour_key(now_ms / 1000),
        sort: format!("{now_ms:013}#{origin}.r"),
        uid: original.uid.clone(),
        records: original.record.clone(),
        seeds: vec![item.seed],
        battles: 1,
        held: false,
        ttl: now_ms / 1000 + LOG_TTL,
    };
    ignore_existing(store.append_log(&log))
}

/// The correction of a counted wrong record (§11.3): the original with weight −1 and the
/// majority's record with weight +1, both `uid: system`.
fn correct<S: Store + ?Sized>(
    store: &S,
    item: &VerifyItem,
    wrong: &Run,
    right: &Run,
    origin: &str,
    now_ms: u64,
) -> StoreResult<()> {
    let bad = |e: String| StoreError::Other(format!("verification item {}: {e}", item.sort));
    let (wrong_record, _) = Record::decode(&wrong.record, SYSTEM_UID).map_err(bad)?;
    let (mut right_record, _) = Record::decode(&right.record, SYSTEM_UID).map_err(bad)?;
    right_record.weight = 1;
    let mut records = Vec::new();
    wrong_record.negated().encode(&mut records);
    right_record.encode(&mut records);
    let log = LogItem {
        engine: item.engine,
        hour: hour_key(now_ms / 1000),
        sort: format!("{now_ms:013}#{origin}.c"),
        uid: SYSTEM_UID.to_string(),
        records,
        seeds: vec![item.seed, item.seed],
        battles: 2,
        held: false,
        ttl: now_ms / 1000 + LOG_TTL,
    };
    ignore_existing(store.append_log(&log))
}

fn ignore_existing(result: StoreResult<()>) -> StoreResult<()> {
    match result {
        Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Penalizes a user (§11.3) and returns whether that banned them. A ban deletes all their
/// tokens (they would fail anyway: authentication checks the state); otherwise
/// `rep.recheckOnPenalty` of their recent counted battles get recheck items. The battle
/// with seed `exclude_seed` (the one just corrected) is not rechecked.
pub fn penalize_user<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    engine: u32,
    uid: &str,
    exclude_seed: Option<i64>,
    origin: &str,
    now_ms: u64,
) -> StoreResult<bool> {
    let now = now_ms / 1000;
    let Some(reputation) = update_reputation(store, uid, |r| {
        penalize(r, config, now);
    })?
    else {
        return Ok(false);
    };
    if reputation.state == UserState::Banned {
        store.put_alert(&ban_alert(uid, "confirmed mismatches", now_ms))?;
        for token in store.list_tokens(uid)? {
            store.delete_token(uid, &token.hash)?;
        }
        return Ok(true);
    }
    create_rechecks(store, config, engine, uid, exclude_seed, origin, now_ms)?;
    Ok(false)
}

/// Recheck items for the most recent counted `ok` battles of `uid` in the last
/// [`RECHECK_HOURS`] hourly log partitions (§11.3), newest first. Returns how many.
pub fn create_rechecks<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    engine: u32,
    uid: &str,
    exclude_seed: Option<i64>,
    origin: &str,
    now_ms: u64,
) -> StoreResult<usize> {
    let wanted = config.rep.recheck_on_penalty as usize;
    let now = now_ms / 1000;
    let mut found: Vec<(Record, i64)> = Vec::new();
    let mut last_hour = None;
    'hours: for back in 0..RECHECK_HOURS {
        let hour = hour_key(now.saturating_sub(back * 3600));
        if last_hour == Some(hour) {
            break;
        }
        last_hour = Some(hour);
        for log in store.query_log(engine, hour)?.iter().rev() {
            if log.uid != uid || log.held {
                continue;
            }
            let Ok(records) = Record::decode_all(&log.records, uid) else {
                continue;
            };
            for (record, &seed) in records.iter().zip(&log.seeds).rev() {
                if found.len() >= wanted {
                    break 'hours;
                }
                if record.failed.is_some() || record.weight != 1 || Some(seed) == exclude_seed {
                    continue;
                }
                found.push((record.clone(), seed));
            }
        }
    }
    for (k, (record, seed)) in found.iter().enumerate() {
        let item = VerifyItem {
            engine,
            sort: format!("{now_ms:013}#{origin}.rc{k:02}"),
            division: record.division,
            seed: *seed,
            bots: record.contestants.iter().map(|e| e.vid).collect(),
            rounds: config.divisions.get(record.division).rounds,
            runs: vec![Run {
                uid: uid.to_string(),
                hash: None,
                record: record.to_bytes(),
                held: false,
                colors: Vec::new(),
            }],
            uids: vec![uid.to_string()],
            stage: 1,
            claimant: None,
            claim_until: None,
            kind: VerifyKind::Recheck,
            ttl: now + VERIFY_TTL,
        };
        ignore_existing(store.put_verify(&item))?;
    }
    Ok(found.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Division;
    use crate::record::Entry;
    use crate::store::{MemStore, TokenItem, TokenKind, User};

    const NOW_MS: u64 = 1_792_843_200_000; // 2026-10-24T12:00:00Z

    fn config() -> Config {
        Config::default()
    }

    fn add_user(store: &MemStore, uid: &str, rep: u32, state: UserState) {
        store
            .create_user(&User {
                uid: uid.to_string(),
                reputation: Reputation {
                    rep,
                    state,
                    ..Reputation::default()
                },
                ..User::default()
            })
            .unwrap();
    }

    fn rep_of(store: &MemStore, uid: &str) -> Reputation {
        store.get_user(uid).unwrap().unwrap().reputation
    }

    fn record(uid: &str, score: f32) -> Record {
        Record {
            division: Division::OneVsOne,
            weight: 1,
            uid: uid.to_string(),
            contestants: vec![
                Entry {
                    vid: 1,
                    score,
                    firsts: 20,
                },
                Entry {
                    vid: 2,
                    score: 1000.0,
                    firsts: 15,
                },
            ],
            failed: None,
            trusted: false,
        }
    }

    fn run(uid: &str, hash: &str, score: f32, held: bool) -> Run {
        Run {
            uid: uid.to_string(),
            hash: Some(hash.to_string()),
            record: record(uid, score).to_bytes(),
            held,
            colors: Vec::new(),
        }
    }

    fn check_item(original: Run) -> VerifyItem {
        VerifyItem {
            engine: 1,
            sort: "0000000000001#b0.0".to_string(),
            division: Division::OneVsOne,
            seed: 77,
            bots: vec![1, 2],
            rounds: 35,
            uids: vec![original.uid.clone()],
            runs: vec![original],
            stage: 1,
            claimant: None,
            claim_until: None,
            kind: VerifyKind::Check,
            ttl: 0,
        }
    }

    fn claim(store: &MemStore, uid: &str) {
        let taken = take_work(store, 1, uid, 5, NOW_MS / 1000 + 3600, NOW_MS / 1000).unwrap();
        assert_eq!(taken.len(), 1, "{uid} gets the item");
    }

    fn resolve_as(store: &MemStore, uid: &str, hash: &str, score: f32) -> Outcome {
        claim(store, uid);
        resolve(
            store,
            &config(),
            1,
            "0000000000001#b0.0",
            run(uid, hash, score, false),
            &format!("b-{uid}.0"),
            NOW_MS,
        )
        .unwrap()
    }

    #[test]
    fn rates_and_tiers() {
        let c = config();
        assert!((verify_rate(0, &c) - 0.20).abs() < 1e-12);
        assert!((verify_rate(37, &c) - 0.2 * (-37.0f64 / 40.0).exp()).abs() < 1e-12);
        assert!((verify_rate(37, &c) - 0.079).abs() < 0.001);
        assert_eq!(verify_rate(120, &c), 0.01);
        assert_eq!(verify_rate(1000, &c), 0.01);
        let mut r = Reputation::default();
        assert_eq!(tier(&r, &c), Tier::New);
        r.rep = 50;
        assert_eq!(tier(&r, &c), Tier::Verifier);
        assert!(is_verifier(&r, &c));
        r.state = UserState::Quarantined;
        assert_eq!(tier(&r, &c), Tier::Quarantined);
        assert!(!is_verifier(&r, &c));
        r.state = UserState::Banned;
        assert_eq!(tier(&r, &c), Tier::Banned);
        assert_ne!(reporter_id("g1"), reporter_id("g2"));
        assert_eq!(reporter_id("g1"), reporter_id("g1"));
    }

    fn colors(body: &str) -> Option<crate::api::BotColors> {
        Some(crate::api::BotColors {
            body: Some(body.to_string()),
            ..Default::default()
        })
    }

    fn add_versions(store: &MemStore) {
        for vid in [1, 2] {
            store
                .put_version(&crate::store::VersionItem {
                    vid,
                    bot: format!("a.Bot{vid}"),
                    version: "1".into(),
                    sha256: "ab".into(),
                    size: 1,
                    codesize: None,
                    class: crate::ids::WeightClass::Mega,
                    divisions: vec![Division::OneVsOne],
                    state: crate::store::VersionState::Active,
                    probation_battles: 0,
                    probation_failures: 0,
                    class_name: String::new(),
                    created: 0,
                    retired: None,
                    url: None,
                    colors: None,
                })
                .unwrap();
        }
    }

    fn colors_of(store: &MemStore, vid: u32) -> Option<crate::api::BotColors> {
        store.get_version(vid).unwrap().unwrap().colors
    }

    /// Resolves the check item as `uid`, with the colours its run reports.
    fn resolve_colored(
        store: &MemStore,
        uid: &str,
        hash: &str,
        colors: Vec<Option<crate::api::BotColors>>,
    ) -> Outcome {
        claim(store, uid);
        resolve(
            store,
            &config(),
            1,
            "0000000000001#b0.0",
            Run {
                colors,
                ..run(uid, hash, 1500.0, false)
            },
            &format!("b-{uid}.0"),
            NOW_MS,
        )
        .unwrap()
    }

    #[test]
    fn only_colours_two_runs_agree_on_are_kept() {
        let store = MemStore::new();
        add_versions(&store);
        for uid in ["g1", "g2", "g3", "g4"] {
            add_user(&store, uid, 60, UserState::New);
        }
        // The original run alone sets nothing.
        let original = Run {
            colors: vec![colors("#000000"), colors("#112233")],
            ..run("g1", "h", 1500.0, false)
        };
        store.put_verify(&check_item(original.clone())).unwrap();
        assert_eq!(colors_of(&store, 1), None);
        // A matching run: the colours both report are kept, not those only one does.
        let outcome = resolve_colored(&store, "g2", "h", vec![colors("#000000"), None]);
        assert_eq!(outcome, Outcome::Matched);
        assert_eq!(colors_of(&store, 1), colors("#000000"));
        assert_eq!(colors_of(&store, 2), None);

        // Another confirmed battle with other colours (a robot that changes them): the
        // first stay; the other bot's are new.
        store.put_verify(&check_item(original.clone())).unwrap();
        let outcome = resolve_colored(
            &store,
            "g3",
            "h",
            vec![colors("#ffffff"), colors("#112233")],
        );
        assert_eq!(outcome, Outcome::Matched);
        assert_eq!(colors_of(&store, 1), colors("#000000"));
        assert_eq!(colors_of(&store, 2), colors("#112233"));
    }

    #[test]
    fn a_majority_confirms_colours_and_a_mismatch_doesnt() {
        let store = MemStore::new();
        add_versions(&store);
        for uid in ["g1", "g2", "g3"] {
            add_user(&store, uid, 60, UserState::New);
        }
        let reported = vec![colors("#123456"), colors("#abcdef")];
        let original = Run {
            colors: reported.clone(),
            ..run("g1", "wrong", 1500.0, false)
        };
        store.put_verify(&check_item(original)).unwrap();
        // A different battle hash: nothing kept yet.
        let outcome = resolve_colored(&store, "g2", "right", reported.clone());
        assert_eq!(outcome, Outcome::TieBreak);
        assert_eq!(colors_of(&store, 1), None);
        // The third run agrees with the second: theirs count.
        let outcome = resolve_colored(&store, "g3", "right", reported);
        assert!(matches!(outcome, Outcome::Majority { .. }), "{outcome:?}");
        assert_eq!(colors_of(&store, 1), colors("#123456"));
        assert_eq!(colors_of(&store, 2), colors("#abcdef"));
    }

    #[test]
    fn stage_one_match() {
        let store = MemStore::new();
        add_user(&store, "g1", 3, UserState::New);
        add_user(&store, "g2", 60, UserState::New);
        store
            .put_verify(&check_item(run("g1", "h", 2000.0, false)))
            .unwrap();
        assert_eq!(resolve_as(&store, "g2", "h", 2000.0), Outcome::Matched);
        assert_eq!(rep_of(&store, "g1").rep, 4);
        assert_eq!(rep_of(&store, "g1").verified_matches, 1);
        assert_eq!(rep_of(&store, "g2").rep, 61);
        assert!(store.verify_items(1).is_empty());
        // Not held: nothing to release, and the verifier's run isn't counted again.
        assert!(store.log_items(1).is_empty());
    }

    #[test]
    fn unclaimed_or_foreign_runs_are_ignored() {
        let store = MemStore::new();
        add_user(&store, "g1", 3, UserState::New);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 60, UserState::New);
        store
            .put_verify(&check_item(run("g1", "h", 2000.0, false)))
            .unwrap();
        let sort = "0000000000001#b0.0";
        // Not claimed by g2.
        let outcome = resolve(
            &store,
            &config(),
            1,
            sort,
            run("g2", "h", 2000.0, false),
            "x",
            NOW_MS,
        );
        assert_eq!(outcome.unwrap(), Outcome::Ignored);
        // Claimed by g3, posted by g2.
        claim(&store, "g3");
        let outcome = resolve(
            &store,
            &config(),
            1,
            sort,
            run("g2", "h", 2000.0, false),
            "x",
            NOW_MS,
        );
        assert_eq!(outcome.unwrap(), Outcome::Ignored);
        assert_eq!(store.verify_items(1).len(), 1);
        // The claimant gives it back.
        release_claim(&store, 1, sort, "g3").unwrap();
        assert!(store.verify_items(1)[0].claimant.is_none());
        // Its own runner never gets it.
        assert!(take_work(&store, 1, "g1", 5, 0, 0).unwrap().is_empty());
    }

    #[test]
    fn take_work_reads_past_pages_of_own_runs() {
        let store = MemStore::new();
        // The oldest three pages are g1's own runs; then two of g2's.
        for i in 0..3 * QUEUE_PAGE + 2 {
            let uid = if i < 3 * QUEUE_PAGE { "g1" } else { "g2" };
            let mut item = check_item(run(uid, "h", 2000.0, false));
            item.sort = format!("{i:013}#b{i}.0");
            store.put_verify(&item).unwrap();
        }
        let taken = take_work(&store, 1, "g1", 5, 100, 0).unwrap();
        let sorts: Vec<_> = taken.iter().map(|i| i.sort.as_str()).collect();
        assert_eq!(sorts, ["0000000000075#b75.0", "0000000000076#b76.0"]);
        // Beyond QUEUE_PAGES pages it gives up.
        let store = MemStore::new();
        for i in 0..QUEUE_PAGES * QUEUE_PAGE + 1 {
            let uid = if i < QUEUE_PAGES * QUEUE_PAGE {
                "g1"
            } else {
                "g2"
            };
            let mut item = check_item(run(uid, "h", 2000.0, false));
            item.sort = format!("{i:013}#b{i}.0");
            store.put_verify(&item).unwrap();
        }
        assert!(take_work(&store, 1, "g1", 5, 100, 0).unwrap().is_empty());
    }

    #[test]
    fn mismatch_then_majority_penalizes_the_counted_original() {
        let store = MemStore::new();
        add_user(&store, "g1", 30, UserState::New);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 70, UserState::New);
        store
            .put_verify(&check_item(run("g1", "bad", 9999.0, false)))
            .unwrap();
        assert_eq!(resolve_as(&store, "g2", "good", 2000.0), Outcome::TieBreak);
        let item = &store.verify_items(1)[0];
        assert_eq!(item.stage, 2);
        assert_eq!(item.runs.len(), 2);
        assert!(item.claimant.is_none());
        assert_eq!(item.uids, ["g1", "g2"]);
        // Neither original nor first verifier can break the tie.
        assert!(take_work(&store, 1, "g2", 5, 0, 0).unwrap().is_empty());

        let outcome = resolve_as(&store, "g3", "good", 2000.0);
        assert_eq!(
            outcome,
            Outcome::Majority {
                minority: "g1".to_string(),
                penalized: Some("g1".to_string()),
                corrected: true,
                banned: false
            }
        );
        let g1 = rep_of(&store, "g1");
        assert_eq!(
            (g1.rep, g1.state, g1.mismatches),
            (0, UserState::Quarantined, 1)
        );
        assert_eq!(g1.mismatch_times, [NOW_MS / 1000]);
        assert_eq!(rep_of(&store, "g2").rep, 61);
        assert_eq!(rep_of(&store, "g3").rep, 71);
        // The correction: -1 of the original, +1 of the majority, uid system.
        let logs = store.log_items(1);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].uid, SYSTEM_UID);
        let records = Record::decode_all(&logs[0].records, SYSTEM_UID).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].weight, -1);
        assert_eq!(records[0].contestants[0].score, 9999.0);
        assert_eq!(records[1].weight, 1);
        assert_eq!(records[1].contestants[0].score, 2000.0);
        assert_eq!(logs[0].seeds, [77, 77]);
    }

    #[test]
    fn majority_penalizes_a_wrong_verifier() {
        let store = MemStore::new();
        add_user(&store, "g1", 30, UserState::New);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 70, UserState::New);
        store
            .put_verify(&check_item(run("g1", "good", 2000.0, false)))
            .unwrap();
        assert_eq!(resolve_as(&store, "g2", "bad", 1.0), Outcome::TieBreak);
        let outcome = resolve_as(&store, "g3", "good", 2000.0);
        assert_eq!(
            outcome,
            Outcome::Majority {
                minority: "g2".to_string(),
                penalized: Some("g2".to_string()),
                corrected: false,
                banned: false
            }
        );
        assert_eq!(rep_of(&store, "g1").rep, 31);
        assert_eq!(rep_of(&store, "g2").state, UserState::Quarantined);
        assert_eq!(rep_of(&store, "g3").rep, 71);
        assert!(store.log_items(1).is_empty(), "the counted result stands");
    }

    #[test]
    fn three_way_disagreement_alerts_without_penalty() {
        let store = MemStore::new();
        add_user(&store, "g1", 30, UserState::New);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 70, UserState::New);
        store
            .put_verify(&check_item(run("g1", "x", 2000.0, false)))
            .unwrap();
        resolve_as(&store, "g2", "y", 2000.0);
        let outcome = resolve_as(&store, "g3", "z", 2000.0);
        assert_eq!(
            outcome,
            Outcome::Disagreement {
                log: "ALERT engine-mismatch 0000000000001#b0.0".to_string()
            }
        );
        let alerts = store.list_alerts().unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "engine-mismatch");
        assert_eq!(alerts[0].reference, "0000000000001#b0.0");
        for (uid, rep) in [("g1", 30), ("g2", 60), ("g3", 70)] {
            let r = rep_of(&store, uid);
            assert_eq!((r.rep, r.state, r.mismatches), (rep, UserState::New, 0));
        }
        assert!(store.verify_items(1).is_empty());
        assert!(store.log_items(1).is_empty(), "the counted result stays");
    }

    #[test]
    fn quarantine_release_and_return() {
        let store = MemStore::new();
        let mut c = config();
        c.rep.quarantine_matches = 2;
        add_user(&store, "q", 0, UserState::Quarantined);
        add_user(&store, "v", 60, UserState::New);
        for n in 0..2u64 {
            let mut item = check_item(run("q", "h", 2000.0, true));
            item.sort = format!("000000000000{n}#q");
            store.put_verify(&item).unwrap();
            claim(&store, "v");
            let outcome = resolve(
                &store,
                &c,
                1,
                &item.sort,
                run("v", "h", 2000.0, false),
                &format!("b{n}.0"),
                NOW_MS + n,
            )
            .unwrap();
            assert_eq!(outcome, Outcome::Matched);
            let q = rep_of(&store, "q");
            if n == 0 {
                assert_eq!(
                    (q.state, q.quarantine_matches, q.rep),
                    (UserState::Quarantined, 1, 0)
                );
            } else {
                assert_eq!(
                    (q.state, q.quarantine_matches, q.rep),
                    (UserState::New, 0, 0)
                );
            }
        }
        // Both held records were released with the original's uid.
        let logs = store.log_items(1);
        assert_eq!(logs.len(), 2);
        for log in &logs {
            assert_eq!(log.uid, "q");
            assert!(!log.held);
            assert_eq!(log.seeds, [77]);
            assert_eq!(Record::decode_all(&log.records, "q").unwrap().len(), 1);
        }
        assert_eq!(rep_of(&store, "v").rep, 62);
    }

    #[test]
    fn a_wrong_held_record_is_not_released_or_corrected() {
        let store = MemStore::new();
        add_user(&store, "q", 0, UserState::Quarantined);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 70, UserState::New);
        store
            .put_verify(&check_item(run("q", "bad", 1.0, true)))
            .unwrap();
        resolve_as(&store, "g2", "good", 2000.0);
        let outcome = resolve_as(&store, "g3", "good", 2000.0);
        assert!(matches!(
            outcome,
            Outcome::Majority {
                corrected: false,
                ..
            }
        ));
        assert!(store.log_items(1).is_empty());
        assert_eq!(rep_of(&store, "q").mismatches, 1);
    }

    #[test]
    fn a_held_record_proven_right_is_released() {
        let store = MemStore::new();
        add_user(&store, "q", 0, UserState::Quarantined);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 70, UserState::New);
        store
            .put_verify(&check_item(run("q", "good", 2000.0, true)))
            .unwrap();
        resolve_as(&store, "g2", "bad", 1.0);
        resolve_as(&store, "g3", "good", 2000.0);
        let logs = store.log_items(1);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].uid, "q");
        assert_eq!(rep_of(&store, "q").quarantine_matches, 1);
    }

    fn log_battles(store: &MemStore, uid: &str, hour_offset: u64, seeds: &[i64]) {
        let at = NOW_MS / 1000 - hour_offset * 3600;
        let mut records = Vec::new();
        for (k, _) in seeds.iter().enumerate() {
            record(uid, 1000.0 + k as f32).encode(&mut records);
        }
        store
            .append_log(&LogItem {
                engine: 1,
                hour: hour_key(at),
                sort: format!("{:013}#b-{uid}-{hour_offset}", at * 1000),
                uid: uid.to_string(),
                records,
                seeds: seeds.to_vec(),
                battles: seeds.len() as u32,
                held: false,
                ttl: 0,
            })
            .unwrap();
    }

    #[test]
    fn penalty_creates_rechecks_of_recent_battles() {
        let store = MemStore::new();
        let mut c = config();
        c.rep.recheck_on_penalty = 4;
        add_user(&store, "g1", 30, UserState::New);
        add_user(&store, "g2", 60, UserState::New);
        add_user(&store, "g3", 70, UserState::New);
        log_battles(&store, "g1", 50, &[1]); // too old
        log_battles(&store, "g1", 5, &[10, 11, 12]);
        log_battles(&store, "g1", 0, &[20, 77]); // 77: the battle being corrected
        log_battles(&store, "other", 0, &[30]);
        store
            .put_verify(&check_item(run("g1", "bad", 9999.0, false)))
            .unwrap();
        for (uid, hash) in [("g2", "good"), ("g3", "good")] {
            claim(&store, uid);
            resolve(
                &store,
                &c,
                1,
                "0000000000001#b0.0",
                run(uid, hash, 2000.0, false),
                &format!("b-{uid}.0"),
                NOW_MS,
            )
            .unwrap();
        }
        let rechecks: Vec<VerifyItem> = store
            .verify_items(1)
            .into_iter()
            .filter(|i| i.kind == VerifyKind::Recheck)
            .collect();
        let seeds: Vec<i64> = rechecks.iter().map(|i| i.seed).collect();
        assert_eq!(
            seeds,
            [20, 12, 11, 10],
            "newest first, the corrected one left out"
        );
        for item in &rechecks {
            assert_eq!(item.runs.len(), 1);
            assert_eq!(item.runs[0].uid, "g1");
            assert!(item.runs[0].hash.is_none());
            assert_eq!(item.uids, ["g1"]);
            assert_eq!(item.bots, [1, 2]);
            assert_eq!(item.rounds, 35);
        }

        // A verifier reproduces one: matched by record, no hash needed.
        let first = &rechecks[0];
        claim_sort(&store, "g2", &first.sort);
        let mut good = run("g2", "hash", 1000.0, false);
        good.record = Record::decode(&first.runs[0].record, "g2")
            .unwrap()
            .0
            .to_bytes();
        let outcome = resolve(&store, &c, 1, &first.sort, good, "x.1", NOW_MS).unwrap();
        assert_eq!(outcome, Outcome::Matched);

        // Another one turns out wrong: corrected, but no second penalty.
        let second = &rechecks[1];
        claim_sort(&store, "g2", &second.sort);
        let outcome = resolve(
            &store,
            &c,
            1,
            &second.sort,
            run("g2", "r", 5.0, false),
            "x.2",
            NOW_MS,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::TieBreak);
        claim_sort(&store, "g3", &second.sort);
        let outcome = resolve(
            &store,
            &c,
            1,
            &second.sort,
            run("g3", "r", 5.0, false),
            "x.3",
            NOW_MS,
        )
        .unwrap();
        assert_eq!(
            outcome,
            Outcome::Majority {
                minority: "g1".to_string(),
                penalized: None,
                corrected: true,
                banned: false
            }
        );
        assert_eq!(rep_of(&store, "g1").mismatches, 1);
    }

    fn claim_sort(store: &MemStore, uid: &str, sort: &str) {
        store
            .claim_verify(1, sort, uid, NOW_MS / 1000 + 3600, NOW_MS / 1000)
            .unwrap();
    }

    #[test]
    fn ban_after_repeated_mismatches() {
        let store = MemStore::new();
        let c = config();
        add_user(&store, "g1", 30, UserState::New);
        for (n, label) in ["phone", "desktop"].iter().enumerate() {
            store
                .put_token(&TokenItem {
                    hash: format!("{n:064}"),
                    uid: "g1".to_string(),
                    kind: TokenKind::Upload,
                    label: label.to_string(),
                    created: 0,
                    last_used: 0,
                    expires: None,
                })
                .unwrap();
        }
        let day = 86_400_000;
        // An old mismatch outside the 90-day window doesn't count.
        let mut times = Vec::new();
        for at in [NOW_MS - 100 * day, NOW_MS - 10 * day, NOW_MS - day] {
            let banned = penalize_user(&store, &c, 1, "g1", None, "o", at).unwrap();
            times.push(banned);
        }
        assert_eq!(times, [false, false, false]);
        assert_eq!(rep_of(&store, "g1").mismatch_times.len(), 2);
        assert_eq!(store.list_tokens("g1").unwrap().len(), 2);
        assert!(penalize_user(&store, &c, 1, "g1", None, "o", NOW_MS).unwrap());
        let g1 = rep_of(&store, "g1");
        assert_eq!((g1.state, g1.mismatches), (UserState::Banned, 4));
        assert!(store.list_tokens("g1").unwrap().is_empty());
        // A banned user earns nothing.
        let mut r = g1.clone();
        credit(&mut r, &c);
        assert_eq!(r, g1);
    }

    #[test]
    fn concurrent_reputation_changes_are_not_lost() {
        let store = MemStore::new();
        add_user(&store, "g1", 0, UserState::New);
        // Simulate a concurrent write between read and write: bump the version.
        let user = store.get_user("g1").unwrap().unwrap();
        store
            .set_reputation(
                "g1",
                0,
                &Reputation {
                    rep: 10,
                    ..user.reputation.clone()
                },
            )
            .unwrap();
        assert_eq!(
            store.set_reputation("g1", 0, &user.reputation),
            Err(StoreError::ConditionFailed)
        );
        update_reputation(&store, "g1", |r| r.rep += 1).unwrap();
        assert_eq!(rep_of(&store, "g1").rep, 11);
        assert_eq!(
            update_reputation(&store, "nobody", |r| r.rep += 1).unwrap(),
            None
        );
    }
}
