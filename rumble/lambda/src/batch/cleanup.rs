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
//!
//! **Purges** (`POST admin/purge`, a `purge` alert) work the same way for every counted
//! record with one of the purge's versions, up to its hour: first the verification items
//! with one of them are deleted, then the log is walked down to the oldest version's
//! registration ([`purge_corrections`]).

use super::{Batch, Pace, READ_RATE, WRITE_RATE, read_units};
use crate::routes::{jar_key, plan_key};
use crate::s3::{Bucket, Fetched};
use rumble_core::Config;
use rumble_core::cleanup::recording_group;
use rumble_core::cleanup::{
    CleanupState, PURGE_ALERT, Pending, PurgeCursor, ban_corrections, ban_cursor, correction_items,
    correction_prefix, dead_versions, died_before_rumble, doomed, parse_purge_reference,
    previous_hour, purge_correction_prefix, purge_corrections, purge_cursor, version_dir,
};
use rumble_core::ids::Division;
use rumble_core::plan::Plan;
use rumble_core::recordings::{entry_key, legacy_entries, listing, version_names};
use rumble_core::store::{LogItem, StoreError, UserState, VersionItem, VersionState, hour_key};
use rumble_core::verify::BAN_ALERT;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

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
    /// Slot entries without a robot order that were reopened (M6) and their tracks deleted.
    pub slots_reopened: usize,
    pub reopened_tracks_deleted: usize,
    pub jars_deleted: usize,
    pub invalidated: Vec<String>,
    pub bans: Vec<BanReport>,
    #[serde(default)]
    pub purges: Vec<PurgeReport>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    pub id: String,
    pub corrected: u64,
    pub next: u32,
    pub finished: bool,
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
    dead_versions_cleanup(batch, engine, &key, &mut state, &mut report, start_ms)?;
    batch.write_state_json(&key, &state)?;
    bans(batch, engine, &key, &mut state, &mut report, start_ms)?;
    batch.write_state_json(&key, &state)?;
    purges(batch, engine, &key, &mut state, &mut report, start_ms)?;
    batch.write_state_json(&key, &state)?;
    // Last: it takes what time the ban removals and purges left.
    reopen_without_order(batch, engine, &mut state, &mut report, start_ms)?;
    batch.write_state_json(&key, &state)?;
    Ok(report)
}

/// M6's one-time migration (spec §12.5): slot entries without a robot order can't be shown,
/// so each filled or claimed entry that has a recording and no `v` loses its entry (the slot
/// is open again) and its track. Found through the listing of `rec/e<E>/` (a group with a
/// track), so a claim in a group without any track stays: it expires and is claimed again,
/// with an order. Once the
/// migration is `done` (see [`RecOrderMigration`]) the job does nothing and reads nothing.
fn reopen_without_order(
    batch: &Batch,
    engine: u32,
    state: &mut CleanupState,
    report: &mut CleanupReport,
    start_ms: u64,
) -> Result<(), String> {
    if state.rec_order.done {
        return Ok(());
    }
    let now = start_ms / 1000;
    state.rec_order.begin(now);
    let store = batch.store();
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    let names = version_names(&versions);
    let keys = batch
        .objects()
        .list(Bucket::Jars, &format!("rec/e{engine}/"))?;
    let listed = listing(&keys, engine, &versions, None);
    drop(keys);
    let mut pace = Pace::new(batch, READ_RATE);
    let mut complete = true;
    for (group, objects) in listed {
        if batch.now_ms().saturating_sub(start_ms) > batch.budget_ms {
            complete = false;
            break;
        }
        pace.take(0.5);
        let division = group.division();
        let Some(item) = store
            .get_slots(engine, division, &group.key())
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let legacy = legacy_entries(&item);
        if legacy.is_empty() {
            continue;
        }
        // The entries first, then the tracks: nobody can claim a slot that still has an entry.
        // A lost race (the entry changed) is left for the next pass.
        ignore_condition(store.remove_slots(engine, division, &group.key(), &legacy))?;
        let after = store
            .get_slots(engine, division, &group.key())
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        for (id, _) in legacy
            .iter()
            .filter(|(id, _)| !after.entries.contains_key(id))
        {
            report.slots_reopened += 1;
            if let Some(track) = entry_key(engine, id, &names).filter(|k| objects.contains(k)) {
                batch.objects().delete(Bucket::Jars, &track)?;
                report.reopened_tracks_deleted += 1;
            }
        }
    }
    if complete {
        state.rec_order.complete(now);
    }
    Ok(())
}

/// Dead versions cleaned per step: their slot items are looked up 1.5 RCU each, paced, so a
/// chunk takes about a minute at most; the cleaned set is saved after each.
const DEAD_CHUNK: usize = 5;

/// Cleans the dead versions within the time budget, a chunk at a time, saving the cleaned set
/// after each chunk: a backlog (2026-10-26: 3,793 retired versions from the archives) is
/// worked off over several runs instead of timing out every run. Versions that died before
/// the rumble began are only marked ([`died_before_rumble`]).
fn dead_versions_cleanup(
    batch: &Batch,
    engine: u32,
    key: &str,
    state: &mut CleanupState,
    report: &mut CleanupReport,
    start_ms: u64,
) -> Result<(), String> {
    let store = batch.store();
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    let (historic, recent): (Vec<&VersionItem>, Vec<&VersionItem>) =
        dead_versions(&versions, state)
            .into_iter()
            .partition(|v| died_before_rumble(v));
    if !historic.is_empty() {
        for version in &historic {
            state.cleaned.insert(version.vid);
        }
        println!(
            "cleanup: {} versions that died before the rumble began marked cleaned",
            historic.len()
        );
        batch.write_state_json(key, state)?;
    }
    if recent.is_empty() {
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
    // At least a chunk a run, then up to half the budget, checked between chunks (a chunk is
    // short: about 40 s paced), so a backlog leaves the ban removals and purges their time.
    let over = || batch.now_ms().saturating_sub(start_ms) > batch.budget_ms / 2;
    let mut paths = Vec::new();
    let mut pace = Pace::new(batch, READ_RATE);
    for (n, chunk) in recent.chunks(DEAD_CHUNK).enumerate() {
        if n > 0 && over() {
            break;
        }
        let (recordings, slots) = doomed(&keys, engine, &versions, chunk, &top, &slot_pairs);
        let removed_dirs: Vec<String> = chunk
            .iter()
            .filter(|v| v.state == VersionState::Removed)
            .map(|v| format!("/{}/", version_dir(v)))
            .collect();
        for key in &recordings {
            batch.objects().delete(Bucket::Jars, key)?;
            report.recordings_deleted += 1;
            if removed_dirs.iter().any(|d| key.contains(d.as_str())) {
                paths.push(format!("/{key}"));
            }
        }
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
        for version in chunk.iter().filter(|v| v.state == VersionState::Removed) {
            let jar = jar_key(&version.sha256);
            if batch.objects().exists(Bucket::Jars, &jar)? {
                batch.objects().delete(Bucket::Jars, &jar)?;
                report.jars_deleted += 1;
                paths.push(format!("/{jar}"));
            }
        }
        for version in chunk {
            state.cleaned.insert(version.vid);
            report.versions_cleaned.push(version.vid);
        }
        batch.write_state_json(key, state)?;
    }
    if !paths.is_empty() {
        batch
            .services
            .cdn
            .invalidate(&paths, &format!("cleanup-{start_ms}"))?;
        println!("cleanup: invalidated {} paths", paths.len());
    }
    report.invalidated.extend(paths);
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
                &correction_prefix(cursor.started_ms, &uid, hour),
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

fn purges(
    batch: &Batch,
    engine: u32,
    key: &str,
    state: &mut CleanupState,
    report: &mut CleanupReport,
    start_ms: u64,
) -> Result<(), String> {
    let store = batch.store();
    let mut new = Vec::new();
    for alert in store.list_alerts().map_err(|e| e.to_string())? {
        if alert.kind != PURGE_ALERT {
            continue;
        }
        let Some(asked) = parse_purge_reference(&alert.reference) else {
            println!("cleanup: unreadable purge alert {}", alert.sort);
            continue;
        };
        if !state.purged.contains(&asked.id) && !state.purges.contains_key(&asked.id) {
            new.push((asked, alert.text));
        }
    }
    if !new.is_empty() {
        let versions = store.list_versions().map_err(|e| e.to_string())?;
        for (asked, text) in new {
            println!("cleanup: purge {}: {text}", asked.id);
            let cursor = purge_cursor(
                asked.vids,
                &text,
                asked.recordings,
                &versions,
                batch.now_ms(),
            );
            state.purges.insert(asked.id, cursor);
        }
        batch.write_state_json(key, state)?;
    }
    let ids: Vec<String> = state.purges.keys().cloned().collect();
    let mut reads = Pace::new(batch, READ_RATE);
    let mut writes = Pace::new(batch, WRITE_RATE);
    for id in ids {
        let mut cursor: PurgeCursor = state.purges[&id].clone();
        if !cursor.queue_done {
            // No verification of a purged battle may correct it after the purge.
            for item in store
                .query_verify(engine, None, usize::MAX)
                .map_err(|e| e.to_string())?
            {
                if item.bots.iter().any(|v| cursor.vids.contains(v)) {
                    ignore_condition(store.delete_verify(engine, &item.sort, None))?;
                }
            }
            cursor.queue_done = true;
            state.purges.insert(id.clone(), cursor.clone());
            batch.write_state_json(key, state)?;
        }
        if cursor.recordings && !cursor.recordings_done {
            if !all_recordings(batch, engine, start_ms, report)? {
                report.purges.push(PurgeReport {
                    id: id.clone(),
                    corrected: cursor.corrected,
                    next: cursor.next,
                    finished: false,
                });
                continue;
            }
            cursor.recordings_done = true;
            state.purges.insert(id.clone(), cursor.clone());
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
            let corrections = purge_corrections(&mut after, &items);
            after.next = previous_hour(hour);
            if corrections.is_empty() {
                cursor = after;
                unsaved += 1;
                if unsaved >= 100 {
                    state.purges.insert(id.clone(), cursor.clone());
                    batch.write_state_json(key, state)?;
                    unsaved = 0;
                }
                continue;
            }
            let now = batch.now_ms() / 1000;
            let partition = hour_key(now);
            let prefix = purge_correction_prefix(cursor.started_ms, &id, hour);
            let mut items: Vec<LogItem> =
                correction_items(engine, &prefix, partition, &corrections, now);
            if let Some(pending) = cursor.pending.as_ref().filter(|p| p.source == hour) {
                // A run stopped while writing these: keep what landed.
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
            state.purges.insert(id.clone(), cursor.clone());
            batch.write_state_json(key, state)?;
            for item in &items {
                writes.take(1.0);
                ignore_condition(store.append_log(item))?;
            }
            after.pending = None;
            after.corrected += corrections.len() as u64;
            cursor = after;
            state.purges.insert(id.clone(), cursor.clone());
            batch.write_state_json(key, state)?;
            unsaved = 0;
        }
        report.purges.push(PurgeReport {
            id: id.clone(),
            corrected: cursor.corrected,
            next: cursor.next,
            finished: cursor.finished(),
        });
        if cursor.finished() {
            println!(
                "cleanup: purge {id}: {} records corrected",
                cursor.corrected
            );
            state.purges.remove(&id);
            state.purged.insert(id);
        } else {
            state.purges.insert(id, cursor);
        }
    }
    Ok(())
}

/// A purge's recordings (§12.4): every track of the engine version and the slot items they
/// are in, so that the slots fill again; publish's recordings cache, so that the index is
/// built anew. Slot items first (a filled entry without its track would never fill again),
/// then the tracks, then the slot items once more (a slot filled meanwhile opens again; its
/// new track is gone). Each step can be repeated; false: the time budget ran out first.
fn all_recordings(
    batch: &Batch,
    engine: u32,
    start_ms: u64,
    report: &mut CleanupReport,
) -> Result<bool, String> {
    let store = batch.store();
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    let dirs: HashMap<String, u32> = versions.iter().map(|v| (version_dir(v), v.vid)).collect();
    let prefix = format!("rec/e{engine}/");
    let keys = batch.objects().list(Bucket::Jars, &prefix)?;
    let groups: BTreeSet<(Division, String)> = keys
        .iter()
        .filter_map(|k| recording_group(k, engine, &dirs))
        .map(|g| (g.division(), g.key()))
        .collect();
    let over = || batch.now_ms().saturating_sub(start_ms) > batch.budget_ms;
    let mut writes = Pace::new(batch, WRITE_RATE);
    let mut delete_slots = |report: &mut CleanupReport| -> Result<bool, String> {
        for (division, slot_key) in &groups {
            if over() {
                return Ok(false);
            }
            writes.take(1.0);
            store
                .delete_slots(engine, *division, slot_key)
                .map_err(|e| e.to_string())?;
            report.slot_items_deleted += 1;
        }
        Ok(true)
    };
    if !delete_slots(report)? {
        return Ok(false);
    }
    for key in &keys {
        if over() {
            return Ok(false);
        }
        batch.objects().delete(Bucket::Jars, key)?;
        report.recordings_deleted += 1;
    }
    if !delete_slots(report)? {
        return Ok(false);
    }
    for division in [Division::OneVsOne, Division::Melee] {
        batch.objects().delete(
            Bucket::State,
            &rumble_core::recordings::cache_key(engine, division),
        )?;
    }
    if !keys.is_empty() {
        let path = format!("/{prefix}*");
        batch
            .services
            .cdn
            .invalidate(std::slice::from_ref(&path), &format!("purge-{start_ms}"))?;
        report.invalidated.push(path);
    }
    println!(
        "cleanup: purge: {} recordings and {} slot groups deleted",
        keys.len(),
        groups.len()
    );
    Ok(true)
}
