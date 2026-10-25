// SPDX-License-Identifier: EPL-2.0

//! The pure part of the `batch` Lambda's `plan` mode (spec §10.5, §12.3): which hours of the
//! result log to fold, folding a log partition into the division states, the version
//! updates after folding (probation counts, probation and failure transitions, retiring the
//! previous version when probation passes), the probation work (`V#` items of kind
//! `probation`), the plan's bots and `top` from the published snapshots.
//!
//! The Lambda reads and writes the objects and items; the order that makes a run crash-safe
//! is: fold, write `plan.json.gz`, then `pairings.bin` (the state is its own cursor: a crash
//! before it is written folds the same hours again into the old state), then the version
//! updates (recomputed from the state by the next run if they didn't happen).

use crate::config::Config;
use crate::data;
use crate::ids::{Division, RumbleId, WeightClass};
use crate::plan::{self, PlanBot, Transition, VersionStatus};
use crate::publish::Snapshot;
use crate::record::Record;
use crate::results::Random;
use crate::state::{State, VersionTally};
use crate::store::{
    ALERT_TTL, Alert, BotState, DAY, LogItem, SYSTEM_UID, Store, StoreError, StoreResult,
    VERIFY_TTL, VerifyItem, VerifyKind, VersionItem, VersionState, hour_key,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// A post's time is taken before its write lands: an hour is folded only once it ended at
/// least this long ago (§10.5).
pub const GRACE_SECONDS: u64 = 600;
/// Without a state file, folding starts this many hours before the last foldable hour. No
/// work is handed out before the first plan of an engine exists, so there is nothing older;
/// the margin only covers a state file lost by hand.
pub const FIRST_LOOKBACK_HOURS: u64 = 24;

/// The division's index in per-division arrays (`Division::ALL` order).
pub fn division_index(division: Division) -> usize {
    match division {
        Division::OneVsOne => 0,
        Division::Melee => 1,
    }
}

/// The unix time an hour key (`YYYYMMDDHH`, UTC) starts; `None` if it is not one.
pub fn hour_start(hour: u32) -> Option<u64> {
    let (date, h) = (hour / 100, hour % 100);
    let text = format!(
        "{:04}-{:02}-{:02}",
        date / 10_000,
        date / 100 % 100,
        date % 100
    );
    let day = u64::try_from(data::day_number(&text)?).ok()?;
    let start = day * DAY + u64::from(h) * 3600;
    (h < 24 && hour_key(start) == hour).then_some(start)
}

/// The last hour that ended at least [`GRACE_SECONDS`] before `now`.
pub fn last_foldable_hour(now: u64) -> u32 {
    hour_key(now.saturating_sub(3600 + GRACE_SECONDS))
}

/// The `done` of a new state: [`FIRST_LOOKBACK_HOURS`] before the last foldable hour.
pub fn initial_done(now: u64) -> u32 {
    hour_key(now.saturating_sub(3600 + GRACE_SECONDS + FIRST_LOOKBACK_HOURS * 3600))
}

/// The hours after `done` up to the last foldable one, oldest first (§10.5 step 1).
pub fn hours_to_fold(done: u32, now: u64) -> Vec<u32> {
    let last = last_foldable_hour(now);
    let Some(mut at) = hour_start(done) else {
        return Vec::new();
    };
    let mut hours = Vec::new();
    loop {
        at += 3600;
        let hour = hour_key(at);
        if hour > last {
            return hours;
        }
        hours.push(hour);
    }
}

/// Days since 1970-01-01 of a unix time, as the pairing state stores them.
pub fn day_of(unix: u64) -> u16 {
    (unix / DAY).min(u64::from(u16::MAX)) as u16
}

/// What folding one hour found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HourReport {
    /// Counted battles per division (`Division::ALL` order): the weights of the hour's
    /// counted (not failed, not held) records, whether or not a state folded them.
    pub battles: [i64; 2],
    /// Records folded into some state.
    pub folded: u64,
    /// Log items skipped because they are held (a quarantined user's).
    pub held: u64,
    /// Every uid that posted in the hour, except `system`.
    pub uids: BTreeSet<String>,
    /// Items that didn't decode (logged, skipped).
    pub errors: Vec<String>,
}

/// Folds the log partition of `hour` into each state that hasn't folded it yet (`done <
/// hour`) and moves those states' `done` to it (§10.5 step 1). Held items are skipped
/// (released records come back as items of their own); a record is folded with the item's
/// uid as its reporter.
pub fn fold_hour(states: &mut [State], hour: u32, items: &[LogItem]) -> HourReport {
    let mut report = HourReport::default();
    let day = hour_start(hour).map(day_of).unwrap_or(0);
    for item in items {
        if item.uid != SYSTEM_UID {
            report.uids.insert(item.uid.clone());
        }
        if item.held {
            report.held += 1;
            continue;
        }
        let records = match Record::decode_all(&item.records, &item.uid) {
            Ok(records) => records,
            Err(e) => {
                report
                    .errors
                    .push(format!("log item {hour} {}: {e}", item.sort));
                continue;
            }
        };
        for record in &records {
            if record.failed.is_none() {
                report.battles[division_index(record.division)] += i64::from(record.weight);
            }
            for state in states.iter_mut() {
                if state.done < hour && state.division == record.division {
                    state.fold_from(record, day, &item.uid);
                    report.folded += 1;
                }
            }
        }
    }
    for state in states.iter_mut() {
        if state.done < hour {
            state.done = hour;
        }
    }
    report
}

/// The versions whose pairings leave the state (§10.5 step 2): retired, removed, failed.
pub fn dropped_versions(versions: &[VersionItem]) -> HashSet<u32> {
    versions
        .iter()
        .filter(|v| {
            matches!(
                v.state,
                VersionState::Retired | VersionState::Removed | VersionState::Failed
            )
        })
        .map(|v| v.vid)
        .collect()
}

fn plan_bot(version: &VersionItem) -> PlanBot {
    PlanBot {
        vid: version.vid,
        id: version.bot.clone(),
        version: version.version.clone(),
        sha256: version.sha256.clone(),
        size: version.size,
        class: version.class,
    }
}

/// The division's planned (active) and probation versions, by vid.
pub fn plan_bots(versions: &[VersionItem], division: Division) -> (Vec<PlanBot>, Vec<PlanBot>) {
    let of = |state: VersionState| -> Vec<PlanBot> {
        versions
            .iter()
            .filter(|v| v.state == state && v.divisions.contains(&division))
            .map(plan_bot)
            .collect()
    };
    (of(VersionState::Active), of(VersionState::Probation))
}

/// The vids of active versions ranked in the top `top` of one of the snapshots (§10.5
/// `top`). Snapshots name bots by id and version.
pub fn top_from_snapshots(snapshots: &[Snapshot], versions: &[VersionItem], top: u32) -> Vec<u32> {
    let by_name: HashMap<(&str, &str), u32> = versions
        .iter()
        .filter(|v| v.state == VersionState::Active)
        .map(|v| ((v.bot.as_str(), v.version.as_str()), v.vid))
        .collect();
    let mut vids: BTreeSet<u32> = BTreeSet::new();
    for snapshot in snapshots {
        for (id, row) in snapshot {
            if row.0 <= top
                && let Some(&vid) = by_name.get(&(id.as_str(), row.3.as_str()))
            {
                vids.insert(vid);
            }
        }
    }
    vids.into_iter().collect()
}

/// The active versions of a division best first, from the general rumble's snapshot if there
/// is one (ranked versions first, by rank; the rest by vid).
pub fn ranking(
    snapshot: Option<&Snapshot>,
    versions: &[VersionItem],
    division: Division,
) -> Vec<u32> {
    let mut active: Vec<(u32, u32)> = versions
        .iter()
        .filter(|v| v.state == VersionState::Active && v.divisions.contains(&division))
        .map(|v| {
            let rank = snapshot
                .and_then(|s| s.get(&v.bot))
                .filter(|row| row.3 == v.version)
                .map_or(u32::MAX, |row| row.0);
            (rank, v.vid)
        })
        .collect();
    active.sort_unstable();
    active.into_iter().map(|(_, vid)| vid).collect()
}

/// The tally of a version summed over the divisions' states (a version may be in both).
fn merged_tally(states: &[State], vid: u32) -> VersionTally {
    let mut sum = VersionTally::default();
    for state in states {
        if let Some(t) = state.versions.get(&vid) {
            sum.battles += t.battles;
            sum.failed += t.failed;
        }
    }
    sum
}

/// A change of a version for `REG`: the new item, the state it must still have, and the
/// transition if its state changes.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionUpdate {
    pub version: VersionItem,
    pub expected: VersionState,
    pub transition: Option<Transition>,
}

fn version_state(status: VersionStatus) -> VersionState {
    match status {
        VersionStatus::Probation => VersionState::Probation,
        VersionStatus::Active => VersionState::Active,
        VersionStatus::Failed => VersionState::Failed,
    }
}

/// The updates of `REG` after folding (§10.5 step 4, §12.3): probation versions get their
/// counts (`pb`, `pf`, summed over the divisions) and pass or fail after
/// `probation.battles`; active versions whose recent battles failed (in either division,
/// by at least 3 reporters) fail. Versions whose item wouldn't change are left out.
pub fn version_updates(
    states: &[State],
    config: &Config,
    versions: &[VersionItem],
) -> Vec<VersionUpdate> {
    let mut updates = Vec::new();
    for version in versions {
        match version.state {
            VersionState::Probation => {
                let tally = merged_tally(states, version.vid);
                let mut merged = State::new(0, Division::OneVsOne, 0);
                merged.versions.insert(version.vid, tally);
                let transition =
                    plan::transitions(&merged, config, &[(version.vid, VersionStatus::Probation)])
                        .pop();
                let mut item = version.clone();
                item.probation_battles = tally.battles;
                item.probation_failures = tally.failed;
                if let Some(t) = &transition {
                    item.state = version_state(t.to);
                }
                if item != *version {
                    updates.push(VersionUpdate {
                        version: item,
                        expected: VersionState::Probation,
                        transition,
                    });
                }
            }
            VersionState::Active => {
                let transition = states
                    .iter()
                    .filter(|s| version.divisions.contains(&s.division))
                    .find_map(|s| {
                        plan::transitions(s, config, &[(version.vid, VersionStatus::Active)]).pop()
                    });
                if let Some(t) = transition {
                    let mut item = version.clone();
                    item.state = version_state(t.to);
                    updates.push(VersionUpdate {
                        version: item,
                        expected: VersionState::Active,
                        transition: Some(t),
                    });
                }
            }
            _ => {}
        }
    }
    updates
}

/// Writes the version updates (each conditional on the state it was computed from, so a
/// concurrent retirement or takedown wins) and their consequences (§12.3): a version that
/// passed probation becomes the bot's `cur` and the bot's previous active version is retired;
/// an active version that failed raises an alert. Returns the lines to log and the vids
/// whose probation ended (their remaining probation items can go).
pub fn apply_updates<S: Store + ?Sized>(
    store: &S,
    updates: &[VersionUpdate],
    now_ms: u64,
) -> StoreResult<(Vec<String>, Vec<u32>)> {
    let now = now_ms / 1000;
    let mut log = Vec::new();
    let mut ended = Vec::new();
    for update in updates {
        let v = &update.version;
        match store.update_version(v, update.expected) {
            Ok(()) => {}
            Err(StoreError::ConditionFailed) => {
                log.push(format!(
                    "plan: {} {} (vid {}) changed meanwhile; not updated",
                    v.bot, v.version, v.vid
                ));
                continue;
            }
            Err(e) => return Err(e),
        }
        let Some(t) = &update.transition else {
            continue;
        };
        match (t.from, t.to) {
            (VersionStatus::Probation, VersionStatus::Active) => {
                ended.push(v.vid);
                let mut line = format!(
                    "plan: {} {} (vid {}) is active ({})",
                    v.bot, v.version, v.vid, t.reason
                );
                if let Some(mut bot) = store.get_bot(&v.bot)?
                    && bot.state != BotState::Removed
                {
                    if let Some(previous) = bot.current.filter(|&p| p != v.vid)
                        && let Some(mut old) = store.get_version(previous)?
                        && old.state == VersionState::Active
                    {
                        old.state = VersionState::Retired;
                        old.retired = Some(now);
                        match store.update_version(&old, VersionState::Active) {
                            Ok(()) => line
                                .push_str(&format!("; {} (vid {}) retired", old.version, previous)),
                            Err(StoreError::ConditionFailed) => {}
                            Err(e) => return Err(e),
                        }
                    }
                    bot.current = Some(v.vid);
                    store.put_bot(&bot)?;
                }
                log.push(line);
            }
            (VersionStatus::Probation, VersionStatus::Failed) => {
                ended.push(v.vid);
                log.push(format!(
                    "plan: {} {} (vid {}) failed probation ({})",
                    v.bot, v.version, v.vid, t.reason
                ));
            }
            (VersionStatus::Active, VersionStatus::Failed) => {
                let text = format!(
                    "{} {} (vid {}) failed: {}",
                    v.bot, v.version, v.vid, t.reason
                );
                store.put_alert(&Alert {
                    sort: format!("{now_ms:013}#failed.{}", v.vid),
                    kind: "bot-failed".to_string(),
                    text: text.clone(),
                    reference: v.bot.clone(),
                    ttl: now + ALERT_TTL,
                })?;
                log.push(format!("ALERT bot-failed {text}"));
            }
            _ => {}
        }
    }
    Ok((log, ended))
}

/// One issue of probation items for a version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    /// Unix seconds.
    pub at: u64,
    pub division: Division,
    pub count: u32,
}

/// `state/e<E>/probation.json`: the probation items handed to the queue per version, so
/// they are created once (and again only after the earlier ones may have expired).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProbationLedger {
    pub issued: BTreeMap<u32, Vec<Issue>>,
}

/// The sort key of a probation item: the version's creation time first (the queue is
/// served oldest first), then fixed parts, so that recreating an issue after a crash puts
/// the same items (conditional puts make it idempotent).
pub fn probation_sort(version: &VersionItem, round: usize, k: u32) -> String {
    format!(
        "{:013}#prob.{}.{round}.{k}",
        version.created * 1000,
        version.vid
    )
}

impl ProbationLedger {
    /// The sort keys of every probation item issued for `version`.
    pub fn sorts(&self, version: &VersionItem) -> Vec<String> {
        self.issued
            .get(&version.vid)
            .map(|issues| {
                issues
                    .iter()
                    .enumerate()
                    .flat_map(|(round, issue)| {
                        (0..issue.count).map(move |k| probation_sort(version, round, k))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A batch of probation items to create: `round` indexes the version's issues.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbationWork {
    pub vid: u32,
    pub division: Division,
    pub round: usize,
    pub count: u32,
}

/// The probation items to issue now (§12.3): a probation version without an issue gets
/// `probation.battles` minus its counted battles, in its first division; once its last
/// issue is older than the queue's TTL (the items may have expired) it gets the shortfall
/// again. The caller creates the items ([`probation_items`]) and then records the issue.
pub fn probation_work(
    ledger: &ProbationLedger,
    states: &[State],
    config: &Config,
    versions: &[VersionItem],
    now: u64,
) -> Vec<ProbationWork> {
    let mut work = Vec::new();
    for version in versions {
        if version.state != VersionState::Probation {
            continue;
        }
        let Some(&division) = Division::ALL.iter().find(|d| version.divisions.contains(d)) else {
            continue;
        };
        let battles = merged_tally(states, version.vid).battles;
        let need = config.probation.battles.saturating_sub(battles);
        if need == 0 {
            continue;
        }
        let issues = ledger.issued.get(&version.vid).map_or(&[][..], |i| &i[..]);
        let due = match issues.last() {
            None => true,
            Some(last) => last.at + VERIFY_TTL <= now,
        };
        if due {
            work.push(ProbationWork {
                vid: version.vid,
                division,
                round: issues.len(),
                count: need,
            });
        }
    }
    work
}

fn uniform(random: &mut Random) -> f64 {
    (random.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// Opponents of a probation version: `per_battle` distinct versions sampled from the middle
/// half of `ranking` (best first; the whole ranking when the middle is too small), versions
/// of the same bot excluded.
fn opponents(
    ranking: &[u32],
    exclude: &HashSet<u32>,
    per_battle: usize,
    random: &mut Random,
) -> Option<Vec<u32>> {
    let pool: Vec<u32> = ranking
        .iter()
        .copied()
        .filter(|v| !exclude.contains(v))
        .collect();
    if pool.len() < per_battle || per_battle == 0 {
        return None;
    }
    let quarter = pool.len() / 4;
    let mut window: Vec<u32> = if pool.len() - 2 * quarter >= per_battle {
        pool[quarter..pool.len() - quarter].to_vec()
    } else {
        pool
    };
    // Partial Fisher-Yates: the first `per_battle` are a uniform sample.
    for i in 0..per_battle {
        let j =
            i + ((uniform(random) * (window.len() - i) as f64) as usize).min(window.len() - i - 1);
        window.swap(i, j);
    }
    window.truncate(per_battle);
    Some(window)
}

/// The `V#` items of one probation issue (§12.3): each a battle of the version against
/// active versions near the middle of `ranking` (best first), in shuffled robot order with a
/// random seed. Empty if the division has too few other active versions.
#[allow(clippy::too_many_arguments)]
pub fn probation_items(
    config: &Config,
    engine: u32,
    version: &VersionItem,
    work: &ProbationWork,
    ranking: &[u32],
    same_bot: &HashSet<u32>,
    random: &mut Random,
    now: u64,
) -> Vec<VerifyItem> {
    let division = config.divisions.get(work.division);
    let per_battle = division.robots.saturating_sub(1) as usize;
    let mut exclude = same_bot.clone();
    exclude.insert(version.vid);
    let mut items = Vec::new();
    for k in 0..work.count {
        let Some(mut bots) = opponents(ranking, &exclude, per_battle, random) else {
            return Vec::new();
        };
        bots.push(version.vid);
        for i in (1..bots.len()).rev() {
            let j = ((uniform(random) * (i + 1) as f64) as usize).min(i);
            bots.swap(i, j);
        }
        items.push(VerifyItem {
            engine,
            sort: probation_sort(version, work.round, k),
            division: work.division,
            seed: random.next_u64() as i64,
            bots,
            rounds: division.rounds,
            runs: Vec::new(),
            uids: Vec::new(),
            stage: 1,
            claimant: None,
            claim_until: None,
            kind: VerifyKind::Probation,
            ttl: now + VERIFY_TTL,
        });
    }
    items
}

/// `state/users.json`: every uid that ever posted (from the folded log), the list the
/// `publish` job reads users by (the table has no index of users, spec §15 step 6).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UserIndex {
    pub uids: BTreeSet<String>,
}

/// The rumbles of a division whose snapshots give `top` (all four weight classes).
pub fn division_rumbles(division: Division) -> [RumbleId; 4] {
    RumbleId::of_division(division)
}

/// The general (mega) rumble of a division.
pub fn general_rumble(division: Division) -> RumbleId {
    RumbleId::new(division, WeightClass::Mega)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::SnapshotRow;
    use crate::record::{Entry, FailReason};
    use crate::store::{BotItem, BotSource, MemStore};

    const NOW: u64 = 1_792_843_200; // 2026-10-24T12:00:00Z

    fn version(vid: u32, bot: &str, state: VersionState) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.to_string(),
            version: format!("1.{vid}"),
            sha256: format!("{vid:064x}"),
            size: 100,
            codesize: Some(1000),
            class: WeightClass::Mini,
            divisions: vec![Division::OneVsOne],
            state,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.to_string(),
            created: NOW - 1000 + u64::from(vid),
            retired: None,
            url: None,
            colors: None,
        }
    }

    fn record(a: u32, b: u32, failed: Option<u8>) -> Record {
        Record {
            division: Division::OneVsOne,
            weight: 1,
            uid: String::new(),
            contestants: vec![
                Entry {
                    vid: a,
                    score: 60.0,
                    firsts: 6,
                },
                Entry {
                    vid: b,
                    score: 40.0,
                    firsts: 4,
                },
            ],
            failed: failed.map(|r| (r, FailReason::Load)),
            trusted: true,
        }
    }

    fn log(uid: &str, records: &[Record], held: bool) -> LogItem {
        let mut bytes = Vec::new();
        for r in records {
            r.encode(&mut bytes);
        }
        LogItem {
            engine: 1,
            hour: 0,
            sort: format!("1#{uid}"),
            uid: uid.to_string(),
            records: bytes,
            seeds: vec![0; records.len()],
            battles: records.len() as u32,
            held,
            ttl: 0,
        }
    }

    #[test]
    fn hours() {
        assert_eq!(hour_start(2026102412), Some(NOW));
        assert_eq!(hour_start(2026022912), None);
        assert_eq!(hour_start(2026102424), None);
        // At 12:05 the hour 10 ended 65 min ago, 11 only 5 min ago.
        assert_eq!(last_foldable_hour(NOW + 300), 2026102410);
        assert_eq!(last_foldable_hour(NOW + 600), 2026102411);
        assert_eq!(
            hours_to_fold(2026102322, NOW + 300),
            vec![
                2026102323, 2026102400, 2026102401, 2026102402, 2026102403, 2026102404, 2026102405,
                2026102406, 2026102407, 2026102408, 2026102409, 2026102410
            ]
        );
        assert!(hours_to_fold(2026102410, NOW + 300).is_empty());
        assert_eq!(initial_done(NOW + 300), 2026102310);
        assert_eq!(
            hours_to_fold(2025123123, 1_767_225_600 + 4200),
            vec![2026010100]
        );
    }

    #[test]
    fn folding_an_hour_once() {
        let mut states = vec![
            State::new(1, Division::OneVsOne, 2026102409),
            State::new(1, Division::Melee, 2026102410),
        ];
        let items = vec![
            log("g1", &[record(1, 2, None), record(1, 3, Some(1))], false),
            log("g2", &[record(2, 3, None)], true),
            log(SYSTEM_UID, &[record(1, 2, None).negated()], false),
        ];
        let report = fold_hour(&mut states, 2026102410, &items);
        assert_eq!(report.battles, [0, 0]);
        assert_eq!(report.held, 1);
        assert_eq!(report.folded, 3);
        assert_eq!(
            report.uids.iter().cloned().collect::<Vec<_>>(),
            vec!["g1".to_string(), "g2".to_string()]
        );
        assert_eq!(states[0].done, 2026102410);
        assert_eq!(states[0].pairing(1, 2), None, "the correction cancelled it");
        assert_eq!(states[0].versions[&3].failed, 1);
        // Folding the same hour again changes nothing.
        let before = states.clone();
        let again = fold_hour(&mut states, 2026102410, &items);
        assert_eq!(again.folded, 0);
        assert_eq!(states, before);
    }

    #[test]
    fn probation_passes_and_retires_the_previous_version() {
        let store = MemStore::new();
        let config = Config::default();
        let old = version(1, "x.Bot", VersionState::Active);
        let new = version(2, "x.Bot", VersionState::Probation);
        let failing = version(3, "y.Bot", VersionState::Probation);
        let other = version(4, "z.Bot", VersionState::Active);
        for v in [&old, &new, &failing, &other] {
            store.put_version(v).unwrap();
        }
        store
            .put_bot(&BotItem {
                id: "x.Bot".into(),
                author: "Ann".into(),
                owner: None,
                homepage: None,
                description: None,
                divisions: vec![Division::OneVsOne],
                versions: vec![2, 1],
                current: Some(1),
                state: BotState::Active,
                created: NOW,
                source: BotSource::Submit,
            })
            .unwrap();
        let mut state = State::new(1, Division::OneVsOne, 0);
        for _ in 0..9 {
            state.fold(&record(2, 4, None), 1);
            state.fold(&record(3, 4, Some(0)), 1);
        }
        let versions = store.list_versions().unwrap();
        let states = vec![state.clone()];
        // 9 battles: counts only.
        let updates = version_updates(&states, &config, &versions);
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|u| u.transition.is_none()));
        assert_eq!(updates[0].version.probation_battles, 9);
        apply_updates(&store, &updates, NOW * 1000).unwrap();
        assert!(version_updates(&states, &config, &store.list_versions().unwrap()).is_empty());

        state.fold(&record(2, 4, None), 1);
        state.fold(&record(3, 4, Some(0)), 1);
        let updates = version_updates(&[state], &config, &store.list_versions().unwrap());
        let (log, ended) = apply_updates(&store, &updates, NOW * 1000).unwrap();
        assert_eq!(ended, vec![2, 3]);
        assert!(log[0].contains("retired"), "{log:?}");
        assert_eq!(
            store.get_version(1).unwrap().unwrap().state,
            VersionState::Retired
        );
        assert_eq!(store.get_version(1).unwrap().unwrap().retired, Some(NOW));
        assert_eq!(
            store.get_version(2).unwrap().unwrap().state,
            VersionState::Active
        );
        assert_eq!(
            store.get_version(3).unwrap().unwrap().state,
            VersionState::Failed
        );
        assert_eq!(store.get_bot("x.Bot").unwrap().unwrap().current, Some(2));
    }

    #[test]
    fn a_concurrent_takedown_wins() {
        let store = MemStore::new();
        let mut v = version(2, "x.Bot", VersionState::Probation);
        store.put_version(&v).unwrap();
        let mut state = State::new(1, Division::OneVsOne, 0);
        for _ in 0..10 {
            state.fold(&record(2, 4, None), 1);
        }
        let updates = version_updates(&[state], &Config::default(), &[v.clone()]);
        v.state = VersionState::Removed;
        store.put_version(&v).unwrap();
        let (log, ended) = apply_updates(&store, &updates, NOW * 1000).unwrap();
        assert!(ended.is_empty() && log[0].contains("not updated"));
        assert_eq!(
            store.get_version(2).unwrap().unwrap().state,
            VersionState::Removed
        );
    }

    #[test]
    fn probation_items_near_the_middle() {
        let config = Config::default();
        let mut versions: Vec<VersionItem> = (1..=20)
            .map(|vid| version(vid, &format!("b.B{vid}"), VersionState::Active))
            .collect();
        let probation = version(21, "b.B1", VersionState::Probation);
        versions.push(probation.clone());
        let mut snapshot = Snapshot::new();
        for vid in 1..=20u32 {
            // Rank = 21 - vid: vid 20 is best.
            snapshot.insert(
                format!("b.B{vid}"),
                SnapshotRow(21 - vid, 50.0, 1500.0, format!("1.{vid}")),
            );
        }
        let order = ranking(Some(&snapshot), &versions, Division::OneVsOne);
        assert_eq!(order[0], 20);
        assert_eq!(order.len(), 20);

        let ledger = ProbationLedger::default();
        let work = probation_work(&ledger, &[], &config, &versions, NOW);
        assert_eq!(
            work,
            vec![ProbationWork {
                vid: 21,
                division: Division::OneVsOne,
                round: 0,
                count: 10
            }]
        );
        let mut random = Random::new(&[7; 32]);
        let same_bot: HashSet<u32> = [1].into();
        let items = probation_items(
            &config,
            1,
            &probation,
            &work[0],
            &order,
            &same_bot,
            &mut random,
            NOW,
        );
        assert_eq!(items.len(), 10);
        // The pool without the bot's own version 1 has 19 versions: the middle half is 4..15.
        let pool: Vec<u32> = order.iter().copied().filter(|&v| v != 1).collect();
        let middle: HashSet<u32> = pool[4..15].iter().copied().collect();
        for item in &items {
            assert_eq!(item.kind, VerifyKind::Probation);
            assert_eq!(item.bots.len(), 2);
            assert!(item.bots.contains(&21));
            let opponent = item.bots.iter().find(|&&v| v != 21).unwrap();
            assert!(middle.contains(opponent), "{opponent}");
        }
        let mut ledger = ledger;
        ledger.issued.insert(
            21,
            vec![Issue {
                at: NOW,
                division: Division::OneVsOne,
                count: 10,
            }],
        );
        let sorts = ledger.sorts(&probation);
        assert_eq!(
            sorts,
            items.iter().map(|i| i.sort.clone()).collect::<Vec<_>>()
        );
        // Issued: nothing more until the items may have expired.
        assert!(probation_work(&ledger, &[], &config, &versions, NOW + 3600).is_empty());
        let later = probation_work(&ledger, &[], &config, &versions, NOW + VERIFY_TTL);
        assert_eq!((later[0].round, later[0].count), (1, 10));
    }

    #[test]
    fn top_and_plan_bots() {
        let mut versions = vec![
            version(1, "a.A", VersionState::Active),
            version(2, "b.B", VersionState::Active),
            version(3, "c.C", VersionState::Probation),
            version(4, "d.D", VersionState::Retired),
        ];
        versions[1].divisions = vec![Division::OneVsOne, Division::Melee];
        let mut snapshot = Snapshot::new();
        snapshot.insert("a.A".into(), SnapshotRow(1, 60.0, 1600.0, "1.1".into()));
        snapshot.insert("b.B".into(), SnapshotRow(2, 55.0, 1550.0, "0.9".into()));
        snapshot.insert("d.D".into(), SnapshotRow(3, 50.0, 1500.0, "1.4".into()));
        assert_eq!(top_from_snapshots(&[snapshot], &versions, 50), vec![1]);
        let (active, probation) = plan_bots(&versions, Division::OneVsOne);
        assert_eq!(active.iter().map(|b| b.vid).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(probation.iter().map(|b| b.vid).collect::<Vec<_>>(), vec![3]);
        let (active, _) = plan_bots(&versions, Division::Melee);
        assert_eq!(active.len(), 1);
        assert_eq!(dropped_versions(&versions), [4].into());
    }
}
