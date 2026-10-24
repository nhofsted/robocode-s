// SPDX-License-Identifier: EPL-2.0

//! `plan` (spec §10.5): for each hour after the states' `done` up to the last hour that
//! ended at least 10 minutes ago, Query its `R#` partition and fold it into the division
//! states; read `REG` and drop the versions that left; compute and write each division's
//! `plan.json.gz`, then its `pairings.bin` (the cursor: a crash before it is written folds
//! the same hours again into the old state); then the version updates and the probation
//! work. Users seen in the log are added to `state/users.json` (before the states, so a
//! rerun adds them again at worst).

use super::{Batch, Pace, READ_RATE, read_units};
use crate::routes::plan_key;
use crate::s3::{Bucket, Fetched};
use rumble_core::Config;
use rumble_core::batch::{
    self as core, Issue, ProbationLedger, UserIndex, apply_updates, division_index,
    dropped_versions, fold_hour, hours_to_fold, initial_done, last_foldable_hour, plan_bots,
    probation_items, probation_work, ranking, top_from_snapshots, version_updates,
};
use rumble_core::ids::{Division, RumbleId};
use rumble_core::plan::{self, Plan, PlanInput};
use rumble_core::publish::{Snapshot, date_of, shift_date, snapshot_key};
use rumble_core::results::Random;
use rumble_core::state::State;
use rumble_core::store::{LogItem, StoreError, VersionItem, rfc3339};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// `state/users.json` (not per engine: users outlive engine versions).
pub const USERS_KEY: &str = "state/users.json";

pub fn state_key(engine: u32, division: Division) -> String {
    format!("state/e{engine}/{}/pairings.bin", division.as_str())
}

pub fn probation_key(engine: u32) -> String {
    format!("state/e{engine}/probation.json")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DivisionReport {
    pub division: String,
    pub done: u32,
    pub pairings: usize,
    pub battles: u64,
    pub last_hour_battles: u64,
    pub hourly: u32,
    pub bots: usize,
    pub probation: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub engine: u32,
    pub hours: Vec<u32>,
    pub folded: u64,
    pub held: u64,
    pub divisions: Vec<DivisionReport>,
    pub probation_items: usize,
    pub log: Vec<String>,
}

/// What a plan run leaves for `publish`.
pub struct Planned {
    pub report: PlanReport,
    /// In `Division::ALL` order.
    pub states: Vec<State>,
}

fn log_items_bytes(items: &[LogItem]) -> usize {
    items
        .iter()
        .map(|i| i.records.len() + 8 * i.seeds.len() + 100)
        .sum()
}

/// The newest snapshot of a rumble: today's, else yesterday's.
pub fn latest_snapshot(
    batch: &Batch,
    engine: u32,
    rumble: RumbleId,
    now: u64,
) -> Result<Option<Snapshot>, String> {
    let today = date_of(now);
    for date in [Some(today.clone()), shift_date(&today, -1)]
        .into_iter()
        .flatten()
    {
        let key = snapshot_key(engine, &rumble.to_string(), &date);
        if let Some(snapshot) = batch.read_json::<Snapshot>(Bucket::State, &key)? {
            return Ok(Some(snapshot));
        }
    }
    Ok(None)
}

fn load_state(batch: &Batch, engine: u32, division: Division, now: u64) -> Result<State, String> {
    let key = state_key(engine, division);
    match batch.objects().get(Bucket::State, &key, None)? {
        Fetched::Found { bytes, .. } => {
            let state = State::from_bytes(&bytes)?;
            if state.engine != engine || state.division != division {
                return Err(format!(
                    "{key} holds engine {} {}",
                    state.engine, state.division
                ));
            }
            Ok(state)
        }
        _ => Ok(State::new(engine, division, initial_done(now))),
    }
}

fn ignore_condition(result: Result<(), StoreError>) -> Result<(), String> {
    match result {
        Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn run(batch: &Batch, config: &Config) -> Result<PlanReport, String> {
    run_states(batch, config).map(|planned| planned.report)
}

/// The plan run; returns the states as written.
pub fn run_states(batch: &Batch, config: &Config) -> Result<Planned, String> {
    let engine = config.engine_version;
    let now_ms = batch.now_ms();
    let now = now_ms / 1000;
    let store = batch.store();
    let mut report = PlanReport {
        engine,
        ..PlanReport::default()
    };
    let mut pace = Pace::new(batch, READ_RATE);

    // 1. Fold the hours after the states' `done`.
    let mut states = Division::ALL
        .iter()
        .map(|&d| load_state(batch, engine, d, now))
        .collect::<Result<Vec<_>, _>>()?;
    let first_done = states.iter().map(|s| s.done).min().unwrap_or(0);
    let mut users: UserIndex = batch
        .read_json(Bucket::State, USERS_KEY)?
        .unwrap_or_default();
    let users_before = users.clone();
    let mut last_hour = [0i64; 2];
    report.hours = hours_to_fold(first_done, now);
    for &hour in &report.hours {
        let items = store.query_log(engine, hour).map_err(|e| e.to_string())?;
        pace.take(read_units(log_items_bytes(&items)));
        let folded = fold_hour(&mut states, hour, &items);
        report.folded += folded.folded;
        report.held += folded.held;
        users.uids.extend(folded.uids);
        for error in folded.errors {
            println!("ERROR {error}");
        }
        last_hour = folded.battles;
    }
    if report.hours.is_empty() {
        // Nothing new: the last full hour's battles for the target, without folding.
        let hour = last_foldable_hour(now);
        let items = store.query_log(engine, hour).map_err(|e| e.to_string())?;
        last_hour = fold_hour(&mut [], hour, &items).battles;
    }

    // 2. Versions; retired, removed and failed ones leave the states.
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    let dropped = dropped_versions(&versions);
    for state in &mut states {
        state.drop_versions(&dropped);
    }

    // 3. Plans (top from the latest snapshots), then the states.
    let mut snapshots: BTreeMap<RumbleId, Snapshot> = BTreeMap::new();
    for division in Division::ALL {
        for rumble in core::division_rumbles(division) {
            if let Some(snapshot) = latest_snapshot(batch, engine, rumble, now)? {
                snapshots.insert(rumble, snapshot);
            }
        }
    }
    let generated_at = rfc3339(now);
    for state in &states {
        let division = state.division;
        let (bots, probation) = plan_bots(&versions, division);
        let division_snapshots: Vec<Snapshot> = core::division_rumbles(division)
            .iter()
            .filter_map(|r| snapshots.get(r).cloned())
            .collect();
        let top = top_from_snapshots(&division_snapshots, &versions, config.slots.top);
        let slot_pairs = match division {
            Division::OneVsOne => super::slot_pairs::read(batch, engine)?,
            Division::Melee => Vec::new(),
        };
        let last = u64::try_from(last_hour[division_index(division)]).unwrap_or(0);
        let plan: Plan = plan::plan(
            state,
            config,
            &PlanInput {
                bots: &bots,
                probation: &probation,
                top: &top,
                slot_pairs: &slot_pairs,
                last_hour_battles: last,
                generated_at: &generated_at,
            },
        );
        batch.objects().put(
            Bucket::State,
            &plan_key(engine, division),
            &plan.to_bytes(),
            "application/gzip",
            None,
        )?;
        report.divisions.push(DivisionReport {
            division: division.as_str().to_string(),
            done: state.done,
            pairings: state.pairing_count(),
            battles: state.battles,
            last_hour_battles: last,
            hourly: plan.hourly,
            bots: bots.len(),
            probation: probation.len(),
        });
    }
    if users != users_before {
        batch.write_state_json(USERS_KEY, &users)?;
    }
    for state in &states {
        batch.objects().put(
            Bucket::State,
            &state_key(engine, state.division),
            &state.to_bytes(),
            "application/octet-stream",
            None,
        )?;
    }

    // 4. Version updates (recomputed from the state if a crash skips them).
    let updates = version_updates(&states, config, &versions);
    let (log, ended) = apply_updates(store, &updates, now_ms).map_err(|e| e.to_string())?;
    for line in &log {
        println!("{line}");
    }
    report.log.extend(log);

    // 5. Probation work: items of ended probations go, new probation versions get theirs.
    let key = probation_key(engine);
    let mut ledger: ProbationLedger = batch.read_json(Bucket::State, &key)?.unwrap_or_default();
    let before = ledger.clone();
    let by_vid: BTreeMap<u32, &VersionItem> = versions.iter().map(|v| (v.vid, v)).collect();
    for vid in ended {
        if let Some(version) = by_vid.get(&vid) {
            for sort in ledger.sorts(version) {
                ignore_condition(store.delete_verify(engine, &sort, None))?;
            }
        }
        ledger.issued.remove(&vid);
    }
    let mut random = Random::new(&(batch.services.random)());
    for work in probation_work(&ledger, &states, config, &versions, now) {
        let Some(version) = by_vid.get(&work.vid) else {
            continue;
        };
        let snapshot = snapshots.get(&core::general_rumble(work.division));
        let order = ranking(snapshot, &versions, work.division);
        let same_bot: HashSet<u32> = versions
            .iter()
            .filter(|v| v.bot == version.bot)
            .map(|v| v.vid)
            .collect();
        let items = probation_items(
            config,
            engine,
            version,
            &work,
            &order,
            &same_bot,
            &mut random,
            now,
        );
        if items.is_empty() {
            continue;
        }
        for item in &items {
            ignore_condition(store.put_verify(item))?;
        }
        report.probation_items += items.len();
        ledger.issued.entry(work.vid).or_default().push(Issue {
            at: now,
            division: work.division,
            count: work.count,
        });
    }
    if ledger != before {
        batch.write_state_json(&key, &ledger)?;
    }
    Ok(Planned { report, states })
}
