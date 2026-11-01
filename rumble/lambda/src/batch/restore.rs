// SPDX-License-Identifier: EPL-2.0

//! `restore` (one-off, by hand): gives the retired versions back the pairings an older planner
//! dropped (before 0.2.6 the planner dropped retired versions' pairings; now they stay, frozen,
//! and `publish` writes them as version files, §12.4).
//!
//! The log keeps every battle for 400 days, so the job folds the hours from `from` (the
//! engine's first hour of results or earlier; earlier hours are empty and cost one Query each)
//! up to each live state's `done` into fresh states, then replaces every retired version's
//! tallies and pairings in the live states with the fresh ones (`State::restore_versions`) and
//! writes the states. Reserved concurrency 1 keeps the planner out meanwhile; the next plan
//! folds on from the same `done`. Running it again changes nothing.

use super::plan::{load_state, state_key};
use super::{Batch, Pace, READ_RATE, read_units};
use crate::s3::Bucket;
use rumble_core::Config;
use rumble_core::batch::{fold_hour, hour_start, hours_to_fold};
use rumble_core::ids::Division;
use rumble_core::state::State;
use rumble_core::store::{LogItem, VersionState, hour_key};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub from: u32,
    /// The last hour folded per division (`Division::ALL` order): the live states' `done`.
    pub done: Vec<u32>,
    pub hours: usize,
    pub folded: u64,
    pub retired_versions: usize,
    /// Pairings restored per division.
    pub pairings: Vec<usize>,
}

fn items_bytes(items: &[LogItem]) -> usize {
    items
        .iter()
        .map(|i| i.records.len() + 8 * i.seeds.len() + 100)
        .sum()
}

pub fn run(batch: &Batch, config: &Config, from: Option<u32>) -> Result<RestoreReport, String> {
    let from = from.ok_or("restore needs \"from\": the first hour to fold, YYYYMMDDHH")?;
    let start = hour_start(from).ok_or(format!("from {from} is not an hour (YYYYMMDDHH)"))?;
    let engine = config.engine_version;
    let now = batch.now_ms() / 1000;
    let store = batch.store();
    let mut live = Division::ALL
        .iter()
        .map(|&d| load_state(batch, engine, d, now))
        .collect::<Result<Vec<_>, _>>()?;
    // Fresh states that have folded everything before `from`; each folds up to its live
    // state's `done`.
    let before = hour_key(start - 3600);
    let mut fresh: Vec<State> = Division::ALL
        .iter()
        .map(|&d| State::new(engine, d, before))
        .collect();
    let last = live.iter().map(|s| s.done).max().unwrap_or(before);
    let mut report = RestoreReport {
        from,
        done: live.iter().map(|s| s.done).collect(),
        ..RestoreReport::default()
    };
    let mut pace = Pace::new(batch, READ_RATE);
    for hour in hours_to_fold(before, now)
        .into_iter()
        .filter(|&h| h <= last)
    {
        let items = store.query_log(engine, hour).map_err(|e| e.to_string())?;
        pace.take(read_units(items_bytes(&items)));
        for (fresh, live) in fresh.iter_mut().zip(&live) {
            if hour <= live.done {
                let folded = fold_hour(std::slice::from_mut(fresh), hour, &items);
                report.folded += folded.folded;
                for error in folded.errors {
                    println!("ERROR {error}");
                }
            }
        }
        report.hours += 1;
    }
    let retired: HashSet<u32> = store
        .list_versions()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|v| v.state == VersionState::Retired)
        .map(|v| v.vid)
        .collect();
    report.retired_versions = retired.len();
    for (live, fresh) in live.iter_mut().zip(&fresh) {
        report.pairings.push(live.restore_versions(fresh, &retired));
        batch.objects().put(
            Bucket::State,
            &state_key(engine, live.division),
            &live.to_bytes(),
            "application/octet-stream",
            None,
        )?;
    }
    Ok(report)
}
