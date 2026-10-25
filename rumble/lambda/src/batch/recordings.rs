// SPDX-License-Identifier: EPL-2.0

//! The recordings index (M6, `rumble_core::recordings`): `data/v1/rec/<division>/<bot>.json`
//! per bot with filled slots, written by `publish`.
//!
//! **Reads.** One listing of `rec/e<E>/` in the jars bucket (1,000 keys a request; about
//! 700 requests at 700k tracks), then one eventually consistent GetItem (0.5 RCU) for each
//! group whose objects differ from the state cache (`state/e<E>/rec-cache/<division>.json`).
//! A run stops reading at half the time budget (paced at [`READ_RATE`]); the groups left
//! keep their old cache entries and are read by the next run.
//!
//! **Writes.** One ListObjectsV2 of `data/v1/rec/` returns the ETags of the published
//! files; a file is put only when the MD5 of its bytes differs (the ETag of a single-part
//! object, as for the bot files), and files of bots that no longer have recordings are
//! deleted.

use super::{Batch, Pace, READ_RATE, json_bytes};
use crate::s3::{Bucket, etag_matches};
use rumble_core::ids::Division;
use rumble_core::publish::DATA_CACHE;
use rumble_core::recordings::{
    Cache, build, cache_key, current_vids, index_key, index_prefix, listing, read_group,
    version_names,
};
use rumble_core::store::{BotItem, VersionItem};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingsReport {
    /// Groups (slot items) with recordings of current versions.
    pub groups: usize,
    /// Read from the table this run.
    pub groups_read: usize,
    /// Left for the next run (time budget).
    pub groups_pending: usize,
    pub files: usize,
    pub files_written: usize,
    pub files_unchanged: usize,
    pub files_deleted: usize,
    /// Rounds listed in all files (each 1v1 round appears in two).
    pub rounds: u64,
}

/// What the bot files need: recorded rounds by bot and division name.
pub type Counts = BTreeMap<String, BTreeMap<String, u32>>;

const DIVISIONS: [Division; 2] = [Division::OneVsOne, Division::Melee];

pub fn run(
    batch: &Batch,
    engine: u32,
    versions: &[VersionItem],
    bots: &BTreeMap<String, BotItem>,
) -> Result<(Counts, RecordingsReport), String> {
    let start_ms = batch.now_ms();
    let budget = batch.budget_ms / 2;
    let store = batch.store();
    let mut report = RecordingsReport::default();
    let names = version_names(versions);
    let current = current_vids(versions, bots);
    let keys = batch
        .objects()
        .list(Bucket::Jars, &format!("rec/e{engine}/"))?;
    let listed = listing(&keys, engine, versions, Some(&current));
    drop(keys);
    report.groups = listed.len();

    let mut pace = Pace::new(batch, READ_RATE);
    let mut caches: Vec<Cache> = Vec::new();
    for division in DIVISIONS {
        let key = cache_key(engine, division);
        let old: Cache = batch.read_json(Bucket::State, &key)?.unwrap_or_default();
        let mut new = Cache::default();
        for (group, objects) in listed.iter().filter(|(g, _)| g.division() == division) {
            if old.fresh(engine, *group, objects, &names) {
                new.groups
                    .insert(group.key(), old.groups[&group.key()].clone());
                continue;
            }
            if batch.now_ms().saturating_sub(start_ms) > budget {
                report.groups_pending += 1;
                if let Some(slots) = old.groups.get(&group.key()) {
                    new.groups.insert(group.key(), slots.clone());
                }
                continue;
            }
            pace.take(0.5);
            report.groups_read += 1;
            let item = store
                .get_slots(engine, division, &group.key())
                .map_err(|e| e.to_string())?;
            if let Some(item) = item {
                let slots = read_group(engine, *group, &item, objects, &names);
                if !slots.is_empty() {
                    new.groups.insert(group.key(), slots);
                }
            }
        }
        if new != old {
            batch.write_state_json(&key, &new)?;
        }
        caches.push(new);
    }

    let files = build(engine, versions, bots, &caches[0], &caches[1]);
    let existing: BTreeMap<String, String> = batch
        .objects()
        .list_etags(Bucket::Site, &index_prefix())?
        .into_iter()
        .collect();
    let mut counts = Counts::new();
    let mut wanted = BTreeSet::new();
    for ((division, bot), file) in &files {
        let rounds = file.rounds();
        report.rounds += u64::from(rounds);
        counts
            .entry(bot.clone())
            .or_default()
            .insert(division.as_str().to_string(), rounds);
        let key = index_key(*division, bot);
        let bytes = json_bytes(file);
        if existing.get(&key).is_some_and(|e| etag_matches(e, &bytes)) {
            report.files_unchanged += 1;
        } else {
            batch.objects().put(
                Bucket::Site,
                &key,
                &bytes,
                "application/json",
                Some(DATA_CACHE),
            )?;
            report.files_written += 1;
        }
        wanted.insert(key);
    }
    report.files = files.len();
    for key in existing.keys().filter(|k| !wanted.contains(*k)) {
        batch.objects().delete(Bucket::Site, key)?;
        report.files_deleted += 1;
    }
    Ok((counts, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::Services;
    use crate::cloudfront::MemCdn;
    use crate::s3::{MemObjects, Objects};
    use rumble_core::ids::WeightClass;
    use rumble_core::recordings::IndexFile;
    use rumble_core::slots::{Group, SlotId, recording_key};
    use rumble_core::store::{
        BotSource, BotState, MemStore, SlotEntry, SlotStatus, Store, VersionState,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn version(vid: u32, bot: &str) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.to_string(),
            version: "1".into(),
            sha256: format!("{vid:02x}{}", "cd".repeat(31)),
            size: 1,
            codesize: None,
            class: WeightClass::Mega,
            divisions: vec![Division::OneVsOne, Division::Melee],
            state: VersionState::Active,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.to_string(),
            created: 0,
            retired: None,
            url: None,
            colors: None,
        }
    }

    fn bot(id: &str, vid: u32) -> BotItem {
        BotItem {
            id: id.to_string(),
            author: "a".into(),
            owner: None,
            homepage: None,
            description: None,
            divisions: vec![Division::OneVsOne],
            versions: vec![vid],
            current: Some(vid),
            state: BotState::Active,
            created: 0,
            source: BotSource::Import,
        }
    }

    struct Rig {
        store: Arc<MemStore>,
        objects: Arc<MemObjects>,
        clock: Arc<AtomicU64>,
        versions: Vec<VersionItem>,
        bots: BTreeMap<String, BotItem>,
    }

    impl Rig {
        fn new() -> Rig {
            let versions = vec![version(1, "a.A"), version(2, "b.B"), version(3, "c.C")];
            let bots = versions
                .iter()
                .map(|v| (v.bot.clone(), bot(&v.bot, v.vid)))
                .collect();
            Rig {
                store: Arc::new(MemStore::new()),
                objects: Arc::new(MemObjects::new()),
                clock: Arc::new(AtomicU64::new(0)),
                versions,
                bots,
            }
        }

        fn batch(&self, tick: u64) -> Batch {
            let clock = self.clock.clone();
            Batch::new(Services {
                store: Box::new(self.store.clone()),
                objects: Box::new(self.objects.clone()),
                cdn: Box::new(Arc::new(MemCdn::new())),
                clock: Box::new(move || clock.fetch_add(tick, Ordering::SeqCst)),
                sleep: Box::new(|_| {}),
                random: Box::new(|| [0; 32]),
            })
        }

        /// Fills a slot: the item entry (with `order` unless empty) and its object.
        fn fill(&self, group: Group, suffix: &str, order: Vec<u32>) {
            let slot = SlotId::with_suffix(group, suffix).unwrap();
            let names = version_names(&self.versions);
            let key = recording_key(1, &slot, &|vid| {
                names.get(&vid).map(|(b, _, sha)| (b.clone(), sha.clone()))
            })
            .unwrap();
            self.objects.upload(Bucket::Jars, &key, b"track");
            let id = slot.to_string();
            self.store
                .update_slots(
                    1,
                    group.division(),
                    &group.key(),
                    &[(id.clone(), None)],
                    &[(
                        id,
                        SlotEntry {
                            status: SlotStatus::Filled,
                            batch: "b".into(),
                            index: 0,
                            round: 2,
                            sha256_b64: "x".into(),
                            size: 5,
                            claim_until: 0,
                            seed: 77,
                            order,
                        },
                    )],
                    false,
                )
                .unwrap();
        }

        fn run(&self, tick: u64) -> (Counts, RecordingsReport) {
            run(&self.batch(tick), 1, &self.versions, &self.bots).unwrap()
        }

        fn site_keys(&self) -> Vec<String> {
            self.objects.keys(Bucket::Site)
        }
    }

    #[test]
    fn writes_skips_unchanged_and_deletes_stale() {
        let rig = Rig::new();
        rig.fill(Group::Pair { lo: 1, hi: 2 }, "a.0", vec![1, 2]);
        rig.fill(Group::Pair { lo: 1, hi: 2 }, "b.0", vec![2, 1]);
        // No order (legacy): not indexed.
        rig.fill(Group::Pair { lo: 1, hi: 3 }, "a.0", vec![]);
        rig.fill(Group::Version(2), "w.0", vec![2, 1, 3]);

        let (counts, report) = rig.run(0);
        assert_eq!(report.groups, 3);
        assert_eq!(report.groups_read, 3);
        assert_eq!((report.files, report.files_written), (3, 3));
        assert_eq!(
            rig.site_keys(),
            [
                "data/v1/rec/1v1/a.A.json",
                "data/v1/rec/1v1/b.B.json",
                "data/v1/rec/melee/b.B.json"
            ]
        );
        assert_eq!(counts["a.A"]["1v1"], 2);
        assert_eq!(counts["b.B"]["1v1"], 2);
        assert_eq!(counts["b.B"]["melee"], 1);
        assert!(!counts.contains_key("c.C"));
        let object = rig
            .objects
            .object(Bucket::Site, "data/v1/rec/1v1/a.A.json")
            .unwrap();
        assert_eq!(object.content_type, "application/json");
        assert_eq!(object.cache_control.as_deref(), Some(DATA_CACHE));
        let text = String::from_utf8(object.bytes).unwrap();
        assert!(text.contains(r#"["a-0",2,"77",5,0]"#), "{text}");
        assert!(text.contains(r#""pairs":[{"opp":"b.B""#), "{text}");

        // A second run: nothing is read (the cache), nothing written. Only the legacy
        // group, which has objects the cache lacks, is read again.
        let before: Vec<_> = rig
            .site_keys()
            .iter()
            .map(|k| rig.objects.object(Bucket::Site, k).unwrap().etag)
            .collect();
        let (_, again) = rig.run(0);
        assert_eq!(again.groups_read, 1);
        assert_eq!((again.files_written, again.files_unchanged), (0, 3));
        let after: Vec<_> = rig
            .site_keys()
            .iter()
            .map(|k| rig.objects.object(Bucket::Site, k).unwrap().etag)
            .collect();
        assert_eq!(before, after);

        // A new track changes two files; the third stays.
        rig.fill(Group::Pair { lo: 1, hi: 2 }, "a.1", vec![2, 1]);
        let (counts, changed) = rig.run(0);
        assert_eq!(changed.groups_read, 2);
        assert_eq!((changed.files_written, changed.files_unchanged), (2, 1));
        assert_eq!(counts["a.A"]["1v1"], 3);

        // The group's tracks are gone (reopened): the 1v1 files go.
        for suffix in ["a.0", "a.1", "b.0"] {
            let slot = SlotId::with_suffix(Group::Pair { lo: 1, hi: 2 }, suffix).unwrap();
            let names = version_names(&rig.versions);
            let key = recording_key(1, &slot, &|vid| {
                names.get(&vid).map(|(b, _, sha)| (b.clone(), sha.clone()))
            })
            .unwrap();
            rig.objects.delete(Bucket::Jars, &key).unwrap();
        }
        let (counts, gone) = rig.run(0);
        assert_eq!(gone.files_deleted, 2);
        assert_eq!(rig.site_keys(), ["data/v1/rec/melee/b.B.json"]);
        assert!(!counts.contains_key("a.A"));
    }

    #[test]
    fn only_current_versions_and_the_time_budget() {
        let mut rig = Rig::new();
        rig.fill(Group::Pair { lo: 1, hi: 2 }, "a.0", vec![1, 2]);
        rig.fill(Group::Pair { lo: 2, hi: 3 }, "a.0", vec![2, 3]);
        // c.C moved on to a newer version: its old recordings aren't listed.
        rig.versions.push(version(4, "c.C"));
        rig.bots.get_mut("c.C").unwrap().current = Some(4);
        let (counts, report) = rig.run(0);
        assert_eq!(report.groups, 1);
        assert!(counts.contains_key("a.A") && !counts.contains_key("c.C"));

        // With no time to read, the groups wait for the next run.
        let rig = Rig::new();
        rig.fill(Group::Pair { lo: 1, hi: 2 }, "a.0", vec![1, 2]);
        let slow = rig.batch(10_000_000);
        let (counts, report) = run(&slow, 1, &rig.versions, &rig.bots).unwrap();
        assert_eq!((report.groups_read, report.groups_pending), (0, 1));
        assert!(counts.is_empty());
        let (_, next) = rig.run(0);
        assert_eq!(next.groups_read, 1);
        assert_eq!(next.files_written, 2);
        let file: IndexFile = {
            let object = rig
                .objects
                .object(Bucket::Site, "data/v1/rec/1v1/b.B.json")
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&object.bytes).unwrap();
            assert_eq!(value["pairs"][0]["opp"], "a.A");
            assert_eq!(value["schema"], 1);
            IndexFile::OneVsOne(serde_json::from_value(value).unwrap())
        };
        assert_eq!(file.rounds(), 1);
    }

    #[test]
    fn cleanup_reopens_slots_without_an_order_once() {
        use crate::batch::cleanup;
        use rumble_core::Config;
        let rig = Rig::new();
        for v in &rig.versions {
            rig.store.put_version(v).unwrap();
        }
        let pair = Group::Pair { lo: 1, hi: 2 };
        rig.fill(pair, "a.0", vec![1, 2]);
        rig.fill(pair, "a.1", vec![]);
        rig.fill(pair, "b.0", vec![]);
        rig.fill(Group::Version(3), "l.0", vec![]);
        // An old claim whose upload never came: in a group with tracks it is reopened too.
        let config = Config {
            engine_version: 1,
            ..Config::default()
        };
        rig.store
            .update_slots(
                1,
                Division::OneVsOne,
                &pair.key(),
                &[("1v1.1.2.a.2".into(), None)],
                &[(
                    "1v1.1.2.a.2".into(),
                    SlotEntry {
                        status: SlotStatus::Claimed,
                        batch: "b".into(),
                        index: 0,
                        round: 0,
                        sha256_b64: "x".into(),
                        size: 1,
                        claim_until: 5,
                        seed: 0,
                        order: vec![],
                    },
                )],
                false,
            )
            .unwrap();
        let batch = rig.batch(0);
        rig.clock.store(1_000_000, Ordering::SeqCst);
        let first = cleanup::run(&batch, &config).unwrap();
        assert_eq!(first.slots_reopened, 4, "the claim without a track as well");
        assert_eq!(first.reopened_tracks_deleted, 3);
        let item = rig
            .store
            .get_slots(1, Division::OneVsOne, &pair.key())
            .unwrap()
            .unwrap();
        assert_eq!(item.entries.keys().collect::<Vec<_>>(), ["1v1.1.2.a.0"]);
        assert!(!item.full);
        let kept = rig.objects.keys(Bucket::Jars);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].ends_with("/b-0.track") || kept[0].ends_with("/a-0.track"));
        // Not done yet: a second pass after the grace period finds nothing and finishes.
        let state: rumble_core::cleanup::CleanupState = batch
            .read_json(Bucket::State, &cleanup::cleanup_key(1))
            .unwrap()
            .unwrap();
        assert_eq!(state.rec_order.since, Some(1000));
        assert!(!state.rec_order.done);
        rig.clock.store(
            (1000 + rumble_core::cleanup::ORDER_GRACE) * 1000,
            Ordering::SeqCst,
        );
        let second = cleanup::run(&batch, &config).unwrap();
        assert_eq!(second.slots_reopened, 0);
        let state: rumble_core::cleanup::CleanupState = batch
            .read_json(Bucket::State, &cleanup::cleanup_key(1))
            .unwrap()
            .unwrap();
        assert!(state.rec_order.done);
        // Done: a late legacy fill is no longer looked for.
        rig.fill(pair, "b.1", vec![]);
        assert_eq!(cleanup::run(&batch, &config).unwrap().slots_reopened, 0);
    }
}
