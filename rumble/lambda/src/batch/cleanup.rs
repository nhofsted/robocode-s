// SPDX-License-Identifier: EPL-2.0

//! `cleanup` (spec §12.4, §11.3, §15): for versions retired, failed or removed since the
//! last run, delete their recordings (one listing of `rec/e<E>/`), their slot items and, for
//! a takedown, the jar, and invalidate those paths in CloudFront; then the ban removals.
//!
//! **Ban removal.** Banned users are found by their `ban` alerts (the table has no users
//! index). For each, the job first deletes the verification items of the user's battles
//! (so no release or correction of them follows), then walks the result log backwards from
//! the hour it started to the user's first hour (at most 400 days), writing for every
//! counted record of the user a correction (weight −1, uid `system`) into the current
//! hour's partition, at most [`WRITE_RATE`] items (≤ 1 KB each) a second. The cursor
//! (`state/e<E>/cleanup.json`) is saved before and after each partition's corrections, so a
//! run that stops (time budget, crash) resumes where it was without writing anything twice.

use super::{Batch, Pace, READ_RATE, WRITE_RATE, read_units};
use crate::routes::{jar_key, plan_key};
use crate::s3::{Bucket, Fetched};
use rumble_core::Config;
use rumble_core::cleanup::{
    CleanupState, Pending, ban_corrections, ban_cursor, correction_items, correction_prefix,
    dead_versions, doomed, previous_hour, version_dir,
};
use rumble_core::ids::Division;
use rumble_core::plan::Plan;
use rumble_core::store::{LogItem, StoreError, UserState, VersionState, hour_key};
use rumble_core::verify::BAN_ALERT;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub fn cleanup_key(engine: u32) -> String {
    format!("state/e{engine}/cleanup.json")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BanReport {
    pub uid: String,
    pub corrected: u64,
    pub next: u32,
    pub finished: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub versions_cleaned: Vec<u32>,
    pub recordings_deleted: usize,
    pub slot_items_deleted: usize,
    pub jars_deleted: usize,
    pub invalidated: Vec<String>,
    pub bans: Vec<BanReport>,
}

fn ignore_condition(result: Result<(), StoreError>) -> Result<(), String> {
    match result {
        Ok(()) | Err(StoreError::ConditionFailed) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn run(batch: &Batch, config: &Config) -> Result<CleanupReport, String> {
    let engine = config.engine_version;
    let start_ms = batch.now_ms();
    let key = cleanup_key(engine);
    let mut state: CleanupState = batch.read_json(Bucket::State, &key)?.unwrap_or_default();
    let mut report = CleanupReport::default();
    dead_versions_cleanup(batch, engine, &mut state, &mut report, start_ms)?;
    batch.write_state_json(&key, &state)?;
    bans(batch, engine, &key, &mut state, &mut report, start_ms)?;
    batch.write_state_json(&key, &state)?;
    Ok(report)
}

fn dead_versions_cleanup(
    batch: &Batch,
    engine: u32,
    state: &mut CleanupState,
    report: &mut CleanupReport,
    start_ms: u64,
) -> Result<(), String> {
    let store = batch.store();
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    let dead = dead_versions(&versions, state);
    if dead.is_empty() {
        return Ok(());
    }
    let keys = batch
        .objects()
        .list(Bucket::Jars, &format!("rec/e{engine}/"))?;
    let (top, slot_pairs) =
        match batch
            .objects()
            .get(Bucket::State, &plan_key(engine, Division::OneVsOne), None)?
        {
            Fetched::Found { bytes, .. } => Plan::from_bytes(&bytes)
                .map(|p| (p.top, p.slot_pairs))
                .unwrap_or_default(),
            _ => Default::default(),
        };
    let (recordings, slots) = doomed(&keys, engine, &versions, &dead, &top, &slot_pairs);
    let removed_dirs: Vec<String> = dead
        .iter()
        .filter(|v| v.state == VersionState::Removed)
        .map(|v| format!("/{}/", version_dir(v)))
        .collect();
    let mut paths = Vec::new();
    for key in &recordings {
        batch.objects().delete(Bucket::Jars, key)?;
        report.recordings_deleted += 1;
        if removed_dirs.iter().any(|d| key.contains(d.as_str())) {
            paths.push(format!("/{key}"));
        }
    }
    let mut pace = Pace::new(batch, READ_RATE);
    for (division, slot_key) in &slots {
        pace.take(1.5);
        if store
            .get_slots(engine, *division, slot_key)
            .map_err(|e| e.to_string())?
            .is_some()
        {
            store
                .delete_slots(engine, *division, slot_key)
                .map_err(|e| e.to_string())?;
            report.slot_items_deleted += 1;
        }
    }
    for version in dead.iter().filter(|v| v.state == VersionState::Removed) {
        let jar = jar_key(&version.sha256);
        if batch.objects().exists(Bucket::Jars, &jar)? {
            batch.objects().delete(Bucket::Jars, &jar)?;
            report.jars_deleted += 1;
            paths.push(format!("/{jar}"));
        }
    }
    if !paths.is_empty() {
        batch
            .services
            .cdn
            .invalidate(&paths, &format!("cleanup-{start_ms}"))?;
        println!("cleanup: invalidated {} paths", paths.len());
    }
    report.invalidated = paths;
    for version in &dead {
        state.cleaned.insert(version.vid);
        report.versions_cleaned.push(version.vid);
    }
    Ok(())
}

fn bans(
    batch: &Batch,
    engine: u32,
    key: &str,
    state: &mut CleanupState,
    report: &mut CleanupReport,
    start_ms: u64,
) -> Result<(), String> {
    let store = batch.store();
    for alert in store.list_alerts().map_err(|e| e.to_string())? {
        let uid = alert.reference;
        if alert.kind != BAN_ALERT || state.banned.contains(&uid) || state.bans.contains_key(&uid) {
            continue;
        }
        match store.get_user(&uid).map_err(|e| e.to_string())? {
            Some(user) if user.reputation.state == UserState::Banned => {
                println!("cleanup: removing the battles of banned user {uid}");
                state.bans.insert(uid, ban_cursor(&user, batch.now_ms()));
            }
            _ => {
                // Unbanned meanwhile (or gone): nothing to remove.
                state.banned.insert(uid);
            }
        }
    }
    batch.write_state_json(key, state)?;
    let uids: Vec<String> = state.bans.keys().cloned().collect();
    let mut reads = Pace::new(batch, READ_RATE);
    let mut writes = Pace::new(batch, WRITE_RATE);
    for uid in uids {
        let mut cursor = state.bans[&uid].clone();
        if !cursor.queue_done {
            for item in store
                .query_verify(engine, None, usize::MAX)
                .map_err(|e| e.to_string())?
            {
                if item.runs.first().is_some_and(|r| r.uid == uid) {
                    ignore_condition(store.delete_verify(engine, &item.sort, None))?;
                }
            }
            cursor.queue_done = true;
            state.bans.insert(uid.clone(), cursor.clone());
            batch.write_state_json(key, state)?;
        }
        let mut unsaved = 0;
        while !cursor.finished() {
            if batch.now_ms().saturating_sub(start_ms) > batch.budget_ms {
                break;
            }
            let hour = cursor.next;
            let items = store.query_log(engine, hour).map_err(|e| e.to_string())?;
            reads.take(read_units(
                items
                    .iter()
                    .map(|i| i.records.len() + 8 * i.seeds.len() + 100)
                    .sum(),
            ));
            let mut after = cursor.clone();
            let corrections = ban_corrections(&mut after, &uid, &items);
            after.next = previous_hour(hour);
            if corrections.is_empty() {
                cursor = after;
                unsaved += 1;
                if unsaved >= 100 {
                    state.bans.insert(uid.clone(), cursor.clone());
                    batch.write_state_json(key, state)?;
                    unsaved = 0;
                }
                continue;
            }
            let now = batch.now_ms() / 1000;
            let partition = hour_key(now);
            let mut items: Vec<LogItem> = correction_items(
                engine,
                cursor.started_ms,
                &uid,
                hour,
                partition,
                &corrections,
                now,
            );
            if let Some(pending) = cursor.pending.as_ref().filter(|p| p.source == hour) {
                // A run stopped while writing these: keep what landed.
                let prefix = correction_prefix(cursor.started_ms, &uid, hour);
                let written: HashSet<String> = store
                    .query_log(engine, pending.partition)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|i| i.sort)
                    .filter(|s| s.starts_with(&prefix))
                    .collect();
                items.retain(|i| !written.contains(&i.sort));
            }
            cursor.pending = Some(Pending {
                source: hour,
                partition,
            });
            state.bans.insert(uid.clone(), cursor.clone());
            batch.write_state_json(key, state)?;
            for item in &items {
                writes.take(1.0);
                ignore_condition(store.append_log(item))?;
            }
            after.pending = None;
            after.corrected += corrections.len() as u64;
            cursor = after;
            state.bans.insert(uid.clone(), cursor.clone());
            batch.write_state_json(key, state)?;
            unsaved = 0;
        }
        report.bans.push(BanReport {
            uid: uid.clone(),
            corrected: cursor.corrected,
            next: cursor.next,
            finished: cursor.finished(),
        });
        if cursor.finished() {
            println!(
                "cleanup: banned user {uid}: {} records corrected",
                cursor.corrected
            );
            state.bans.remove(&uid);
            state.banned.insert(uid);
        } else {
            state.bans.insert(uid, cursor);
        }
    }
    Ok(())
}
