// SPDX-License-Identifier: EPL-2.0

//! The recordings index (M6, spec §12.5, `rumble-data.md`): per bot and division one file,
//! `data/v1/rec/<division>/<bot id>.json`, listing the filled recording slots of the bot's
//! current version, with what a viewer needs to fetch a track and name its robots: the
//! round, the seed, the size and the battle's robot order.
//!
//! **Finding the slot items.** Slot items are partitions of their own, so the job lists the
//! recordings (`rec/e<E>/`, whose paths name both versions, see
//! [`crate::cleanup::recording_group`]) and reads only the items of groups that have objects.
//! A filled slot never changes while its versions are current, so the groups read are kept in
//! a [`Cache`] (a state file per division); a group is read again only when its objects are
//! not exactly the cached slots' ([`Cache::fresh`]).

use crate::cleanup::recording_group;
use crate::data::SCHEMA_VERSION;
use crate::ids::{Division, parse_vid36};
use crate::publish::DATA_PREFIX;
use crate::slots::{Group, SlotId, recording_key};
use crate::store::{BotItem, BotState, SlotEntry, SlotItem, SlotStatus, VersionItem};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The index file of a bot in a division.
pub fn index_key(division: Division, bot: &str) -> String {
    format!("{DATA_PREFIX}/rec/{}/{bot}.json", division.as_str())
}

/// The prefix of all index files.
pub fn index_prefix() -> String {
    format!("{DATA_PREFIX}/rec/")
}

/// The cache of a division in the state bucket.
pub fn cache_key(engine: u32, division: Division) -> String {
    format!("state/e{engine}/rec-cache/{}.json", division.as_str())
}

/// A filled slot with its order, as kept in the cache: slot suffix (`a.0`), round, seed,
/// size, robot order (vids).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedSlot(pub String, pub u32, pub i64, pub u64, pub Vec<u32>);

/// `state/e<E>/rec-cache/<division>.json`: the filled slots with an order of every group
/// read, by group key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    pub groups: BTreeMap<String, Vec<CachedSlot>>,
}

/// `vid` → `(bot id, version, jar sha256)`.
pub type Versions = HashMap<u32, (String, String, String)>;

/// Every version's name and hash, for [`recording_key`] and the robots' names.
pub fn version_names(versions: &[VersionItem]) -> Versions {
    versions
        .iter()
        .map(|v| (v.vid, (v.bot.clone(), v.version.clone(), v.sha256.clone())))
        .collect()
}

/// The current versions of the bots that are not removed.
pub fn current_vids(versions: &[VersionItem], bots: &BTreeMap<String, BotItem>) -> HashSet<u32> {
    let known: HashSet<u32> = versions.iter().map(|v| v.vid).collect();
    bots.values()
        .filter(|b| b.state != BotState::Removed)
        .filter_map(|b| b.current)
        .filter(|vid| known.contains(vid))
        .collect()
}

fn key_of(engine: u32, slot: &SlotId, names: &Versions) -> Option<String> {
    recording_key(engine, slot, &|vid| {
        names
            .get(&vid)
            .map(|(bot, _, sha)| (bot.clone(), sha.clone()))
    })
}

/// The recording objects among `keys` by slot group; with `only`, just the groups whose
/// versions are all in it.
pub fn listing(
    keys: &[String],
    engine: u32,
    versions: &[VersionItem],
    only: Option<&HashSet<u32>>,
) -> BTreeMap<Group, BTreeSet<String>> {
    let dirs: HashMap<String, u32> = versions
        .iter()
        .map(|v| (crate::cleanup::version_dir(v), v.vid))
        .collect();
    let mut groups: BTreeMap<Group, BTreeSet<String>> = BTreeMap::new();
    for key in keys {
        let Some(group) = recording_group(key, engine, &dirs) else {
            continue;
        };
        let wanted = |vid: &u32| only.is_none_or(|set| set.contains(vid));
        let all = match group {
            Group::Pair { lo, hi } => wanted(&lo) && wanted(&hi),
            Group::Version(vid) => wanted(&vid),
        };
        if all {
            groups.entry(group).or_default().insert(key.clone());
        }
    }
    groups
}

/// The group of a cache key.
pub fn group_from_key(division: Division, key: &str) -> Option<Group> {
    match division {
        Division::OneVsOne => {
            let (lo, hi) = key.split_once('#')?;
            Some(Group::Pair {
                lo: parse_vid36(lo).ok()?,
                hi: parse_vid36(hi).ok()?,
            })
        }
        Division::Melee => Some(Group::Version(parse_vid36(key).ok()?)),
    }
}

impl Cache {
    /// Whether the cached slots of `group` are exactly the objects `listed`: then the slot
    /// item needs no read. (Objects not cached are new, claimed or without an order; cached
    /// slots without an object were reopened.)
    pub fn fresh(
        &self,
        engine: u32,
        group: Group,
        listed: &BTreeSet<String>,
        names: &Versions,
    ) -> bool {
        let Some(cached) = self.groups.get(&group.key()) else {
            return false;
        };
        let keys: Option<BTreeSet<String>> = cached
            .iter()
            .map(|c| {
                let slot = SlotId::with_suffix(group, &c.0)?;
                key_of(engine, &slot, names)
            })
            .collect();
        keys.as_ref() == Some(listed)
    }
}

/// The slots of `item` that count: filled, with a robot order, and with their object listed.
pub fn read_group(
    engine: u32,
    group: Group,
    item: &SlotItem,
    listed: &BTreeSet<String>,
    names: &Versions,
) -> Vec<CachedSlot> {
    let mut slots: Vec<CachedSlot> = item
        .entries
        .iter()
        .filter(|(_, e)| e.status == SlotStatus::Filled && !e.order.is_empty())
        .filter_map(|(id, e)| {
            let slot = SlotId::parse(id).ok().filter(|s| s.group == group)?;
            listed
                .contains(&key_of(engine, &slot, names)?)
                .then(|| CachedSlot(slot.suffix(), e.round, e.seed, e.size, e.order.clone()))
        })
        .collect();
    slots.sort_by(|a, b| a.0.cmp(&b.0));
    slots
}

/// The entries `cleanup` reopens (M6's one-time migration): filled or claimed without a
/// robot order, whose track therefore can't be shown.
pub fn legacy_entries(item: &SlotItem) -> Vec<(String, SlotEntry)> {
    item.entries
        .iter()
        .filter(|(_, e)| e.order.is_empty())
        .map(|(id, e)| (id.clone(), e.clone()))
        .collect()
}

/// The recording object's key of a legacy entry.
pub fn entry_key(engine: u32, id: &str, names: &Versions) -> Option<String> {
    key_of(engine, &SlotId::parse(id).ok()?, names)
}

// ---- the files ----

/// A 1v1 round: `[file, round, seed, size, me]`. `file` is the track's name without
/// `.track` (`a-0`: the first-sorting bot won); `seed` is a decimal string (64 bits);
/// `me` is this bot's robot index in the track (0 or 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Round(pub String, pub u32, pub String, pub u64, pub u8);

/// A melee round: `[file, round, seed, size, order]`; `order` holds, per robot of the
/// track, an index into the file's `bots`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeleeRound(pub String, pub u32, pub String, pub u64, pub Vec<u32>);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub opp: String,
    pub opp_version: String,
    pub opp_sha16: String,
    pub rounds: Vec<Round>,
}

/// `data/v1/rec/1v1/<bot>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneVsOneFile {
    pub schema: u32,
    pub engine: u32,
    pub bot: String,
    pub version: String,
    pub sha16: String,
    pub pairs: Vec<Pair>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contestant {
    pub bot: String,
    pub version: String,
    pub sha16: String,
}

/// `data/v1/rec/melee/<bot>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeleeFile {
    pub schema: u32,
    pub engine: u32,
    pub bot: String,
    pub version: String,
    pub sha16: String,
    /// The robots of the rounds' orders, by index; the bot itself is among them.
    pub bots: Vec<Contestant>,
    pub rounds: Vec<MeleeRound>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum IndexFile {
    OneVsOne(OneVsOneFile),
    Melee(MeleeFile),
}

impl IndexFile {
    /// Recorded rounds listed.
    pub fn rounds(&self) -> u32 {
        match self {
            IndexFile::OneVsOne(f) => f.pairs.iter().map(|p| p.rounds.len() as u32).sum(),
            IndexFile::Melee(f) => f.rounds.len() as u32,
        }
    }
}

fn sha16(sha: &str) -> String {
    sha.chars().take(16).collect()
}

fn file_stem(key: &str) -> Option<String> {
    key.rsplit('/')
        .next()?
        .strip_suffix(".track")
        .map(str::to_string)
}

/// The index files from the groups' cached slots: `(division, bot id)` → file, for the
/// current version of each bot. In 1v1 both versions of a pair must be current; a melee
/// round whose robots aren't all known versions is left out.
pub fn build(
    engine: u32,
    versions: &[VersionItem],
    bots: &BTreeMap<String, BotItem>,
    one_vs_one: &Cache,
    melee: &Cache,
) -> BTreeMap<(Division, String), IndexFile> {
    let names = version_names(versions);
    let current = current_vids(versions, bots);
    let mut out: BTreeMap<(Division, String), IndexFile> = BTreeMap::new();

    // 1v1: bot → opponent → rounds.
    type Opponents = BTreeMap<String, (String, String, Vec<Round>)>;
    let mut pairs: BTreeMap<String, (u32, Opponents)> = BTreeMap::new();
    for (key, slots) in &one_vs_one.groups {
        let Some(group @ Group::Pair { lo, hi }) = group_from_key(Division::OneVsOne, key) else {
            continue;
        };
        if !current.contains(&lo) || !current.contains(&hi) {
            continue;
        }
        for c in slots {
            let Some(slot) = SlotId::with_suffix(group, &c.0) else {
                continue;
            };
            let (Some(file), [x, y]) = (
                key_of(engine, &slot, &names).and_then(|k| file_stem(&k)),
                c.4.as_slice(),
            ) else {
                continue;
            };
            if !(x == &lo && y == &hi || x == &hi && y == &lo) {
                continue;
            }
            for (me, opp) in [(lo, hi), (hi, lo)] {
                let (bot, _, _) = &names[&me];
                let (opp_bot, opp_version, opp_sha) = &names[&opp];
                let entry = pairs.entry(bot.clone()).or_insert((me, BTreeMap::new()));
                let (_, _, rounds) = entry
                    .1
                    .entry(opp_bot.clone())
                    .or_insert_with(|| (opp_version.clone(), sha16(opp_sha), Vec::new()));
                rounds.push(Round(
                    file.clone(),
                    c.1,
                    c.2.to_string(),
                    c.3,
                    u8::from(*y == me),
                ));
            }
        }
    }
    for (bot, (vid, opponents)) in pairs {
        let (_, version, sha) = &names[&vid];
        let pairs = opponents
            .into_iter()
            .map(|(opp, (opp_version, opp_sha16, mut rounds))| {
                rounds.sort_by(|a, b| a.0.cmp(&b.0));
                Pair {
                    opp,
                    opp_version,
                    opp_sha16,
                    rounds,
                }
            })
            .collect();
        out.insert(
            (Division::OneVsOne, bot.clone()),
            IndexFile::OneVsOne(OneVsOneFile {
                schema: SCHEMA_VERSION,
                engine,
                bot,
                version: version.clone(),
                sha16: sha16(sha),
                pairs,
            }),
        );
    }

    // Melee: one group per version.
    for (key, slots) in &melee.groups {
        let Some(group @ Group::Version(vid)) = group_from_key(Division::Melee, key) else {
            continue;
        };
        if !current.contains(&vid) {
            continue;
        }
        let (bot, version, sha) = &names[&vid];
        let mut table: Vec<u32> = Vec::new();
        let mut rounds = Vec::new();
        for c in slots {
            let Some(slot) = SlotId::with_suffix(group, &c.0) else {
                continue;
            };
            let Some(file) = key_of(engine, &slot, &names).and_then(|k| file_stem(&k)) else {
                continue;
            };
            if !c.4.contains(&vid) || c.4.iter().any(|v| !names.contains_key(v)) {
                continue;
            }
            let order =
                c.4.iter()
                    .map(|v| {
                        let at = table.iter().position(|t| t == v).unwrap_or_else(|| {
                            table.push(*v);
                            table.len() - 1
                        });
                        at as u32
                    })
                    .collect();
            rounds.push(MeleeRound(file, c.1, c.2.to_string(), c.3, order));
        }
        if rounds.is_empty() {
            continue;
        }
        out.insert(
            (Division::Melee, bot.clone()),
            IndexFile::Melee(MeleeFile {
                schema: SCHEMA_VERSION,
                engine,
                bot: bot.clone(),
                version: version.clone(),
                sha16: sha16(sha),
                bots: table
                    .iter()
                    .map(|v| {
                        let (bot, version, sha) = &names[v];
                        Contestant {
                            bot: bot.clone(),
                            version: version.clone(),
                            sha16: sha16(sha),
                        }
                    })
                    .collect(),
                rounds,
            }),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WeightClass;
    use crate::store::{BotSource, VersionState};

    fn version(vid: u32, bot: &str, ver: &str, state: VersionState) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.to_string(),
            version: ver.to_string(),
            sha256: format!("{vid:x}{}", "ab".repeat(31)),
            size: 1,
            codesize: None,
            class: WeightClass::Mega,
            divisions: vec![Division::OneVsOne, Division::Melee],
            state,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.to_string(),
            created: 0,
            retired: None,
            url: None,
        }
    }

    fn bot(id: &str, current: u32) -> BotItem {
        BotItem {
            id: id.to_string(),
            author: "a".into(),
            owner: None,
            homepage: None,
            description: None,
            divisions: vec![Division::OneVsOne],
            versions: vec![current],
            current: Some(current),
            state: BotState::Active,
            created: 0,
            source: BotSource::Import,
        }
    }

    fn world() -> (Vec<VersionItem>, BTreeMap<String, BotItem>) {
        // Bot ids sort z < b? No: "b.B" < "m.M" < "z.Z"; vid 1 is z.Z, so lo (1) sorts last.
        let versions = vec![
            version(1, "z.Z", "1", VersionState::Active),
            version(2, "b.B", "2", VersionState::Active),
            version(3, "m.M", "3", VersionState::Active),
            version(4, "m.M", "0", VersionState::Retired),
        ];
        let bots = [bot("z.Z", 1), bot("b.B", 2), bot("m.M", 3)]
            .into_iter()
            .map(|b| (b.id.clone(), b))
            .collect();
        (versions, bots)
    }

    fn entry(order: Vec<u32>, status: SlotStatus) -> SlotEntry {
        SlotEntry {
            status,
            batch: "b".into(),
            index: 0,
            round: 4,
            sha256_b64: "x".into(),
            size: 99,
            claim_until: 0,
            seed: -7,
            order,
        }
    }

    #[test]
    fn one_vs_one_files() {
        let (versions, bots) = world();
        let names = version_names(&versions);
        let group = Group::Pair { lo: 1, hi: 2 };
        let mut cache = Cache::default();
        // Slot a.0: lo (z.Z) won; ids sort b.B first, so the file letter is b.
        cache.groups.insert(
            group.key(),
            vec![
                CachedSlot("a.0".into(), 3, i64::MIN, 10, vec![2, 1]),
                CachedSlot("b.1".into(), 5, 42, 11, vec![1, 2]),
            ],
        );
        // A pair with a retired version is not listed.
        cache.groups.insert(
            Group::Pair { lo: 1, hi: 4 }.key(),
            vec![CachedSlot("a.0".into(), 0, 0, 1, vec![1, 4])],
        );
        let files = build(7, &versions, &bots, &cache, &Cache::default());
        assert_eq!(files.len(), 2);
        let IndexFile::OneVsOne(z) = &files[&(Division::OneVsOne, "z.Z".to_string())] else {
            panic!()
        };
        assert_eq!((z.engine, z.version.as_str()), (7, "1"));
        assert_eq!(z.sha16, "1ababababababababab"[..16]);
        assert_eq!(z.pairs.len(), 1);
        let pair = &z.pairs[0];
        assert_eq!((pair.opp.as_str(), pair.opp_version.as_str()), ("b.B", "2"));
        assert_eq!(pair.opp_sha16, "2ababababababababab"[..16]);
        assert_eq!(
            pair.rounds,
            vec![
                Round("a-1".into(), 5, "42".into(), 11, 0),
                Round("b-0".into(), 3, i64::MIN.to_string(), 10, 1),
            ]
        );
        let IndexFile::OneVsOne(b) = &files[&(Division::OneVsOne, "b.B".to_string())] else {
            panic!()
        };
        // From b.B's side, the same files, with its own robot index.
        assert_eq!(
            b.pairs[0].rounds,
            vec![
                Round("a-1".into(), 5, "42".into(), 11, 1),
                Round("b-0".into(), 3, i64::MIN.to_string(), 10, 0),
            ]
        );
        assert_eq!(b.pairs[0].opp, "z.Z");
        assert_eq!(files[&(Division::OneVsOne, "z.Z".to_string())].rounds(), 2);
        // The file names are the recording keys' (a = the first-sorting bot won).
        let slot = SlotId::with_suffix(group, "a.0").unwrap();
        assert!(key_of(7, &slot, &names).unwrap().ends_with("/b-0.track"),);
    }

    #[test]
    fn bad_orders_are_skipped() {
        let (versions, bots) = world();
        let mut cache = Cache::default();
        cache.groups.insert(
            Group::Pair { lo: 1, hi: 2 }.key(),
            vec![
                CachedSlot("a.0".into(), 0, 0, 1, vec![1]),
                CachedSlot("a.1".into(), 0, 0, 1, vec![1, 3]),
            ],
        );
        assert!(build(7, &versions, &bots, &cache, &Cache::default()).is_empty());
    }

    #[test]
    fn melee_files() {
        let (versions, bots) = world();
        let mut melee = Cache::default();
        melee.groups.insert(
            Group::Version(3).key(),
            vec![
                CachedSlot("l.0".into(), 1, 5, 20, vec![1, 3, 4]),
                CachedSlot("w.0".into(), 2, 6, 21, vec![3, 2, 1]),
                // Not in the order: skipped.
                CachedSlot("w.1".into(), 2, 6, 21, vec![2, 1]),
            ],
        );
        melee.groups.insert(
            Group::Version(4).key(),
            vec![CachedSlot("w.0".into(), 2, 6, 21, vec![4, 1])],
        );
        let files = build(7, &versions, &bots, &Cache::default(), &melee);
        assert_eq!(files.len(), 1);
        let IndexFile::Melee(m) = &files[&(Division::Melee, "m.M".to_string())] else {
            panic!()
        };
        assert_eq!(
            m.bots
                .iter()
                .map(|c| (c.bot.as_str(), c.version.as_str()))
                .collect::<Vec<_>>(),
            vec![("z.Z", "1"), ("m.M", "3"), ("m.M", "0"), ("b.B", "2")]
        );
        assert_eq!(
            m.rounds,
            vec![
                MeleeRound("l-0".into(), 1, "5".into(), 20, vec![0, 1, 2]),
                MeleeRound("w-0".into(), 2, "6".into(), 21, vec![1, 3, 0]),
            ]
        );
        let json = serde_json::to_string(&files[&(Division::Melee, "m.M".to_string())]).unwrap();
        assert!(json.contains(r#"["w-0",2,"6",21,[1,3,0]]"#), "{json}");
    }

    #[test]
    fn cache_freshness() {
        let (versions, _) = world();
        let names = version_names(&versions);
        let group = Group::Pair { lo: 1, hi: 2 };
        let key = |s: &str| key_of(7, &SlotId::with_suffix(group, s).unwrap(), &names).unwrap();
        let mut cache = Cache::default();
        let listed: BTreeSet<String> = [key("a.0"), key("b.0")].into();
        assert!(!cache.fresh(7, group, &listed, &names));
        cache.groups.insert(
            group.key(),
            vec![
                CachedSlot("a.0".into(), 0, 0, 1, vec![1, 2]),
                CachedSlot("b.0".into(), 0, 0, 1, vec![1, 2]),
            ],
        );
        assert!(cache.fresh(7, group, &listed, &names));
        // A new object, or a deleted one.
        let more: BTreeSet<String> = [key("a.0"), key("b.0"), key("a.1")].into();
        assert!(!cache.fresh(7, group, &more, &names));
        let fewer: BTreeSet<String> = [key("a.0")].into();
        assert!(!cache.fresh(7, group, &fewer, &names));
    }

    #[test]
    fn reading_items() {
        let (versions, _) = world();
        let names = version_names(&versions);
        let group = Group::Pair { lo: 1, hi: 2 };
        let key = |s: &str| key_of(7, &SlotId::with_suffix(group, s).unwrap(), &names).unwrap();
        let mut item = SlotItem::default();
        item.entries
            .insert("1v1.1.2.a.0".into(), entry(vec![1, 2], SlotStatus::Filled));
        // No order (legacy), claimed, and filled but its object isn't there.
        item.entries
            .insert("1v1.1.2.a.1".into(), entry(vec![], SlotStatus::Filled));
        item.entries
            .insert("1v1.1.2.a.2".into(), entry(vec![1, 2], SlotStatus::Claimed));
        item.entries
            .insert("1v1.1.2.b.0".into(), entry(vec![2, 1], SlotStatus::Filled));
        let listed: BTreeSet<String> = [key("a.0"), key("a.1"), key("a.2")].into();
        assert_eq!(
            read_group(7, group, &item, &listed, &names),
            vec![CachedSlot("a.0".into(), 4, -7, 99, vec![1, 2])]
        );
        let legacy: Vec<String> = legacy_entries(&item)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(legacy, vec!["1v1.1.2.a.1"]);
        assert_eq!(entry_key(7, "1v1.1.2.a.1", &names).unwrap(), key("a.1"));
    }

    #[test]
    fn listing_groups() {
        let (versions, bots) = world();
        let names = version_names(&versions);
        let at = |s: &str| {
            key_of(
                7,
                &SlotId::with_suffix(Group::Pair { lo: 1, hi: 2 }, s).unwrap(),
                &names,
            )
            .unwrap()
        };
        let old = key_of(
            7,
            &SlotId::with_suffix(Group::Pair { lo: 1, hi: 4 }, "a.0").unwrap(),
            &names,
        )
        .unwrap();
        let keys = vec![at("a.0"), at("b.1"), old.clone(), "rec/e7/junk".to_string()];
        let all = listing(&keys, 7, &versions, None);
        assert_eq!(all.len(), 2);
        let current = listing(&keys, 7, &versions, Some(&current_vids(&versions, &bots)));
        assert_eq!(current.len(), 1);
        assert_eq!(current[&Group::Pair { lo: 1, hi: 2 }].len(), 2);
        assert_eq!(
            group_from_key(Division::OneVsOne, &Group::Pair { lo: 1, hi: 40 }.key()),
            Some(Group::Pair { lo: 1, hi: 40 })
        );
    }
}
