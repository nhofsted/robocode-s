// SPDX-License-Identifier: EPL-2.0

//! `map` (daily, after a `publish`): the style map of each rumble (`rumble_core::map`) at
//! `data/v1/map/<rumble>.json`. Its members are the published rankings' bots, so the map
//! shows exactly the ranked versions; their pairings come from the division's state as the
//! last `plan` left it. The model of the previous run (`state/e<E>/map/<rumble>.json`) is the
//! starting point, so a run after the first takes seconds and the layout moves little.
//!
//! Reads: one Query of `REG` (vids of the ranked versions); no other table reads. A rumble
//! without a map (too few members with enough pairings) loses its file.

use super::Batch;
use super::plan::load_state;
use crate::s3::Bucket;
use rumble_core::Config;
use rumble_core::data::Rankings;
use rumble_core::ids::{Division, RumbleId};
use rumble_core::map::{self, MapState, map_key, state_key};
use rumble_core::publish::{DATA_CACHE, DATA_PREFIX, rankings_key};
use rumble_core::store::rfc3339;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapReport {
    /// The rumbles with a map.
    pub maps: Vec<MapLine>,
    /// Rumbles without a map (no rankings, or too few bots with enough pairings).
    pub skipped: Vec<String>,
    /// Rumbles left for the next run (time budget).
    pub deferred: Vec<String>,
}

/// One rumble's map in the report.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapLine {
    pub rumble: String,
    /// Bots on the map.
    pub bots: usize,
    pub trustworthiness: f64,
    /// Optimiser iterations: the fit and the layout.
    pub iterations: (usize, usize),
    pub millis: u64,
}

pub fn run(batch: &Batch, config: &Config) -> Result<MapReport, String> {
    let engine = config.engine_version;
    let start_ms = batch.now_ms();
    let now = start_ms / 1000;
    let generated_at = rfc3339(now);
    let mut report = MapReport::default();

    let versions = batch.store().list_versions().map_err(|e| e.to_string())?;
    let vids: HashMap<(&str, &str), u32> = versions
        .iter()
        .map(|v| ((v.bot.as_str(), v.version.as_str()), v.vid))
        .collect();

    for division in Division::ALL {
        let mut state = None;
        for id in RumbleId::of_division(division) {
            let rumble = id.to_string();
            if batch.now_ms() - start_ms > batch.budget_ms {
                report.deferred.push(rumble);
                continue;
            }
            let rankings: Option<Rankings> =
                batch.read_json(Bucket::Site, &rankings_key(&rumble))?;
            let Some(rankings) = rankings.filter(|r| r.engine_version == engine) else {
                report.skipped.push(rumble);
                continue;
            };
            let members: Vec<(String, u32)> = rankings
                .bots
                .iter()
                .filter_map(|row| {
                    let vid = vids.get(&(row.id.as_str(), row.version.as_str()))?;
                    Some((row.id.clone(), *vid))
                })
                .collect();
            if state.is_none() {
                state = Some(load_state(batch, engine, division, now)?);
            }
            let pairings = map::pairings(state.as_ref().expect("loaded"), &members);
            let previous: Option<MapState> =
                batch.read_json(Bucket::State, &state_key(engine, &rumble))?;
            let began = batch.now_ms();
            match map::compute(&pairings, previous.as_ref(), &rumble, &generated_at, engine) {
                Some(run) => {
                    batch.objects().put(
                        Bucket::Site,
                        &map_key(&rumble),
                        &super::json_bytes(&run.map),
                        "application/json",
                        Some(DATA_CACHE),
                    )?;
                    batch.write_state_json(&state_key(engine, &rumble), &run.state)?;
                    report.maps.push(MapLine {
                        rumble,
                        bots: run.map.bots.len(),
                        trustworthiness: run.map.trustworthiness,
                        iterations: run.iterations,
                        millis: batch.now_ms() - began,
                    });
                }
                None => {
                    batch.objects().delete(Bucket::Site, &map_key(&rumble))?;
                    report.skipped.push(rumble);
                }
            }
        }
    }

    batch.services.cdn.invalidate(
        &[format!("/{DATA_PREFIX}/map/*")],
        &format!("map-{start_ms}"),
    )?;
    Ok(report)
}
