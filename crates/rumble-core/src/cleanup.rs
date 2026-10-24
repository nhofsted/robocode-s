// SPDX-License-Identifier: EPL-2.0

//! The pure part of the `batch` Lambda's `cleanup` mode (spec §12.4, §11.3, §15): the
//! recordings and slot items of versions that are no longer current, the jars of removed
//! versions, and the correction records of banned users, with the cursor that makes the
//! ban removal resumable across runs.
//!
//! **Finding slot items without an index.** Slot items are partitions of their own
//! (`S#<E>#<division>#<key>`), so the job derives the keys of a dead version's items from
//! its recordings (a listing of `rec/e<E>/`, whose paths name both versions) and, for
//! claims never uploaded, from the plan's `top` (a slot is only ever claimed when one of the
//! versions is in `top`). A claim that was never uploaded and whose `top` partner left the
//! top before the version died stays behind: a small item nothing reads.

use crate::ids::{Division, vid36};
use crate::record::Record;
use crate::slots::Group;
use crate::store::{DAY, LOG_TTL, LogItem, SYSTEM_UID, User, VersionItem, VersionState, hour_key};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Payload bytes (records and seeds) per correction log item: with the keys and attribute
/// names the item stays below 1 KB, one WCU.
pub const CORRECTION_BYTES: usize = 800;

/// `state/e<E>/cleanup.json`: what the daily job has done.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CleanupState {
    /// Versions whose recordings, slot items (and, if removed, jar) are gone.
    pub cleaned: BTreeSet<u32>,
    /// Ban removals in progress, by uid.
    pub bans: BTreeMap<String, BanCursor>,
    /// Ban removals finished (the alert stays for 90 days; this keeps it from restarting).
    pub banned: BTreeSet<String>,
}

/// The progress of one ban removal: the log is walked backwards, one hourly partition at a
/// time, from the hour the removal started down to the user's first hour (at most the
/// log's 400 days).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BanCursor {
    pub started_ms: u64,
    /// The user's open verification items are deleted (first step).
    pub queue_done: bool,
    /// The next partition to read.
    pub next: u32,
    /// The last partition to read.
    pub stop: u32,
    /// Keys ([`record_key`]) of records already cancelled by a correction seen in a later
    /// hour (a penalized run that was counted), so they are not cancelled twice.
    pub cancelled: Vec<String>,
    /// Corrections of partition `source` being written into partition `partition`: after a
    /// crash the job reads `partition` to see which were written.
    pub pending: Option<Pending>,
    /// Correction records written so far.
    pub corrected: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub source: u32,
    pub partition: u32,
}

/// A version's directory in the recordings' paths (§12.5): `<id>_<sha16>`.
pub fn version_dir(version: &VersionItem) -> String {
    let sha16: String = version.sha256.chars().take(16).collect();
    format!("{}_{sha16}", version.bot)
}

/// Versions that are retired, failed or removed and not cleaned yet.
pub fn dead_versions<'a>(
    versions: &'a [VersionItem],
    state: &CleanupState,
) -> Vec<&'a VersionItem> {
    versions
        .iter()
        .filter(|v| {
            matches!(
                v.state,
                VersionState::Retired | VersionState::Failed | VersionState::Removed
            ) && !state.cleaned.contains(&v.vid)
        })
        .collect()
}

/// The slot group a recording key belongs to, from the versions' directories: 1v1
/// `rec/e<E>/1v1/<dirA>/<dirB>/<file>`, melee `rec/e<E>/melee/<dir>/<file>`.
pub fn recording_group(key: &str, engine: u32, dirs: &HashMap<String, u32>) -> Option<Group> {
    let rest = key.strip_prefix(&format!("rec/e{engine}/"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        ["1v1", a, b, _file] => {
            let (a, b) = (*dirs.get(*a)?, *dirs.get(*b)?);
            Some(Group::Pair {
                lo: a.min(b),
                hi: a.max(b),
            })
        }
        ["melee", dir, _file] => Some(Group::Version(*dirs.get(*dir)?)),
        _ => None,
    }
}

/// What to delete for the dead versions `dead`: the recording keys among `keys` that
/// involve one, and the slot items (division, key) of the dead versions: from those
/// recordings, every pair with a version of `top` (1v1) and the version's own item (melee).
pub fn doomed(
    keys: &[String],
    engine: u32,
    versions: &[VersionItem],
    dead: &[&VersionItem],
    top: &[u32],
) -> (Vec<String>, BTreeSet<(Division, String)>) {
    let dirs: HashMap<String, u32> = versions.iter().map(|v| (version_dir(v), v.vid)).collect();
    let dead_vids: HashSet<u32> = dead.iter().map(|v| v.vid).collect();
    let mut recordings = Vec::new();
    let mut slots = BTreeSet::new();
    for key in keys {
        let Some(group) = recording_group(key, engine, &dirs) else {
            continue;
        };
        let hit = match group {
            Group::Pair { lo, hi } => dead_vids.contains(&lo) || dead_vids.contains(&hi),
            Group::Version(vid) => dead_vids.contains(&vid),
        };
        if hit {
            recordings.push(key.clone());
            slots.insert((group.division(), group.key()));
        }
    }
    for version in dead {
        if version.divisions.contains(&Division::Melee) {
            slots.insert((Division::Melee, vid36(version.vid)));
        }
        if version.divisions.contains(&Division::OneVsOne) {
            for &other in top.iter().filter(|&&t| t != version.vid) {
                let group = Group::Pair {
                    lo: other.min(version.vid),
                    hi: other.max(version.vid),
                };
                slots.insert((Division::OneVsOne, group.key()));
            }
        }
    }
    (recordings, slots)
}

/// The partition before `hour`.
pub fn previous_hour(hour: u32) -> u32 {
    match crate::batch::hour_start(hour) {
        Some(start) => hour_key(start.saturating_sub(3600)),
        None => 0,
    }
}

/// A new ban removal's cursor: from the hour it starts down to the user's creation (or the
/// log's 400 days, whichever is later).
pub fn ban_cursor(user: &User, now_ms: u64) -> BanCursor {
    let now = now_ms / 1000;
    let first = user.created.max(now.saturating_sub(LOG_TTL + DAY));
    BanCursor {
        started_ms: now_ms,
        queue_done: false,
        next: hour_key(now),
        stop: hour_key(first),
        cancelled: Vec::new(),
        pending: None,
        corrected: 0,
    }
}

impl BanCursor {
    pub fn finished(&self) -> bool {
        self.next < self.stop
    }
}

/// The identity of a battle's record across the log: its seed and its binary form with
/// weight +1 (first 16 hex digits of their SHA-256).
pub fn record_key(seed: i64, record: &Record) -> String {
    let mut positive = record.clone();
    positive.weight = 1;
    let mut hasher = Sha256::new();
    hasher.update(seed.to_le_bytes());
    hasher.update(positive.to_bytes());
    hasher.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The records of `uid` in one partition's items that a ban must cancel (§11.3): every
/// counted record of its non-held items (held ones never counted; a released one is an item
/// of its own with the user's uid), except those a system correction in this or a later
/// partition already cancelled. Corrections found here are remembered in the cursor.
pub fn ban_corrections(cursor: &mut BanCursor, uid: &str, items: &[LogItem]) -> Vec<(Record, i64)> {
    for item in items.iter().filter(|i| i.uid == SYSTEM_UID) {
        let Ok(records) = Record::decode_all(&item.records, SYSTEM_UID) else {
            continue;
        };
        for (record, &seed) in records.iter().zip(&item.seeds) {
            if record.weight < 0 {
                cursor.cancelled.push(record_key(seed, record));
            }
        }
    }
    let mut out = Vec::new();
    for item in items.iter().filter(|i| i.uid == uid && !i.held) {
        let Ok(records) = Record::decode_all(&item.records, uid) else {
            continue;
        };
        for (record, &seed) in records.iter().zip(&item.seeds) {
            if record.weight <= 0 {
                continue;
            }
            let key = record_key(seed, record);
            if let Some(at) = cursor.cancelled.iter().position(|k| *k == key) {
                cursor.cancelled.swap_remove(at);
                continue;
            }
            out.push((record.negated(), seed));
        }
    }
    out
}

/// The sort key prefix of the corrections of partition `source` of a ban removal.
pub fn correction_prefix(started_ms: u64, uid: &str, source: u32) -> String {
    format!("{started_ms:013}#ban.{uid}.{source}.")
}

/// The log items (uid `system`) that carry corrections, at most [`CORRECTION_BYTES`] of
/// records and seeds each, for partition `partition`. Their sort keys depend only on the
/// cursor's start, the uid, the source partition and the chunk, so writing them again
/// after a crash is recognized.
pub fn correction_items(
    engine: u32,
    started_ms: u64,
    uid: &str,
    source: u32,
    partition: u32,
    corrections: &[(Record, i64)],
    now: u64,
) -> Vec<LogItem> {
    let prefix = correction_prefix(started_ms, uid, source);
    let mut items: Vec<LogItem> = Vec::new();
    let mut records = Vec::new();
    let mut seeds = Vec::new();
    let flush = |records: &mut Vec<u8>, seeds: &mut Vec<i64>, items: &mut Vec<LogItem>| {
        if seeds.is_empty() {
            return;
        }
        items.push(LogItem {
            engine,
            hour: partition,
            sort: format!("{prefix}{:03}", items.len()),
            uid: SYSTEM_UID.to_string(),
            records: std::mem::take(records),
            battles: seeds.len() as u32,
            seeds: std::mem::take(seeds),
            held: false,
            ttl: now + LOG_TTL,
        });
    };
    for (record, seed) in corrections {
        if !seeds.is_empty()
            && records.len() + record.encoded_len() + 8 * (seeds.len() + 1) > CORRECTION_BYTES
        {
            flush(&mut records, &mut seeds, &mut items);
        }
        record.encode(&mut records);
        seeds.push(*seed);
    }
    flush(&mut records, &mut seeds, &mut items);
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WeightClass;
    use crate::record::Entry;
    use crate::store::{Reputation, UserState};

    const NOW: u64 = 1_792_843_200;

    fn version(vid: u32, bot: &str, state: VersionState) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.to_string(),
            version: "1".into(),
            sha256: format!("{vid:02x}").repeat(32),
            size: 1,
            codesize: None,
            class: WeightClass::Mega,
            divisions: vec![Division::OneVsOne, Division::Melee],
            state,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.to_string(),
            created: NOW,
            retired: None,
            url: None,
        }
    }

    fn record(a: u32, b: u32, score: f32) -> Record {
        Record {
            division: Division::OneVsOne,
            weight: 1,
            uid: String::new(),
            contestants: vec![
                Entry {
                    vid: a,
                    score,
                    firsts: 3,
                },
                Entry {
                    vid: b,
                    score: 100.0 - score,
                    firsts: 2,
                },
            ],
            failed: None,
            trusted: true,
        }
    }

    fn log(uid: &str, records: &[(Record, i64)], held: bool) -> LogItem {
        let mut bytes = Vec::new();
        for (r, _) in records {
            r.encode(&mut bytes);
        }
        LogItem {
            engine: 1,
            hour: 2026102410,
            sort: format!("1#{uid}"),
            uid: uid.to_string(),
            records: bytes,
            seeds: records.iter().map(|(_, s)| *s).collect(),
            battles: records.len() as u32,
            held,
            ttl: 0,
        }
    }

    #[test]
    fn recordings_and_slots_of_dead_versions() {
        let versions = vec![
            version(1, "a.A_x", VersionState::Retired),
            version(2, "b.B", VersionState::Active),
            version(3, "c.C", VersionState::Active),
        ];
        let state = CleanupState::default();
        let dead = dead_versions(&versions, &state);
        assert_eq!(dead.len(), 1);
        let d = |vid: usize| version_dir(&versions[vid - 1]);
        let keys = vec![
            format!("rec/e1/1v1/{}/{}/a-0.track", d(1), d(2)),
            format!("rec/e1/1v1/{}/{}/b-3.track", d(2), d(3)),
            format!("rec/e1/melee/{}/w-0.track", d(1)),
            format!("rec/e2/melee/{}/w-0.track", d(1)),
            "rec/e1/1v1/unknown_00/x/a-0.track".to_string(),
        ];
        let (recordings, slots) = doomed(&keys, 1, &versions, &dead, &[3]);
        assert_eq!(recordings, vec![keys[0].clone(), keys[2].clone()]);
        let slots: Vec<(Division, String)> = slots.into_iter().collect();
        assert_eq!(
            slots,
            vec![
                (Division::OneVsOne, "1#2".to_string()),
                (Division::OneVsOne, "1#3".to_string()),
                (Division::Melee, "1".to_string()),
            ]
        );
    }

    #[test]
    fn ban_corrections_skip_what_was_corrected() {
        let user = User {
            uid: "g7".into(),
            login: "x".into(),
            name: String::new(),
            country: String::new(),
            github_id: 7,
            github_created: None,
            admin: false,
            trusted: false,
            hidden: false,
            bots: vec![],
            battles: 3,
            last_post: None,
            created: NOW - 3 * 3600,
            reputation: Reputation {
                state: UserState::Banned,
                ..Reputation::default()
            },
        };
        let mut cursor = ban_cursor(&user, NOW * 1000);
        assert_eq!((cursor.next, cursor.stop), (2026102412, 2026102409));
        // Newest partition: a correction of the user's record (seed 5).
        let correction = log(
            SYSTEM_UID,
            &[(record(1, 2, 70.0).negated(), 5), (record(1, 2, 40.0), 5)],
            false,
        );
        assert!(ban_corrections(&mut cursor, "g7", &[correction]).is_empty());
        assert_eq!(cursor.cancelled.len(), 1);
        // Older partition: the user's three records, a held item and someone else's.
        let items = vec![
            log(
                "g7",
                &[(record(1, 2, 70.0), 5), (record(1, 3, 60.0), 6)],
                false,
            ),
            log("g7", &[(record(2, 3, 50.0), 7)], true),
            log("g8", &[(record(2, 3, 55.0), 8)], false),
        ];
        let out = ban_corrections(&mut cursor, "g7", &items);
        let mut expected = record(1, 3, 60.0).negated();
        expected.uid = "g7".into();
        assert_eq!(out, vec![(expected, 6)]);
        assert!(cursor.cancelled.is_empty());

        let many: Vec<(Record, i64)> = (0..70).map(|i| (record(1, 3, 50.0).negated(), i)).collect();
        let items = correction_items(1, NOW * 1000, "g7", 2026102410, 2026102412, &many, NOW);
        assert_eq!(items.len(), 3);
        assert!(
            items
                .iter()
                .all(|i| i.records.len() + 8 * i.seeds.len() <= CORRECTION_BYTES)
        );
        assert_eq!(items.iter().map(|i| i.battles).sum::<u32>(), 70);
        assert_eq!(
            items[1].sort,
            format!("{}001", correction_prefix(NOW * 1000, "g7", 2026102410))
        );
        let decoded = Record::decode_all(&items[0].records, SYSTEM_UID).unwrap();
        assert!(decoded.iter().all(|r| r.weight == -1));
        assert_eq!(previous_hour(2026102400), 2026102323);
    }
}
