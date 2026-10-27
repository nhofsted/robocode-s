// SPDX-License-Identifier: EPL-2.0

//! The pure part of the `batch` Lambda's daily `sync` mode (spec §16.5, owner 2026-10-26):
//! keeping the **unclaimed imported** bots (`src: import`, no owner, not removed) in step with
//! the RoboWiki participants lists, as LiteRumble does. Claimed and submitted bots are their
//! owners', removed bots stay down: the sync never touches them.
//!
//! A version's divisions are the lists it is on. Per managed bot and trusted list:
//! - a listed version that is new is imported **active** (its jar fetched and validated by the
//!   Lambda first), the bot's other versions that are listed nowhere retired;
//! - a listed version that is `delisted` is active again with its frozen pairings, one that is
//!   `retired` is active again from scratch (its pairings were dropped);
//! - a version that left one list loses that division; a bot that left every list is
//!   `delisted` (version and bot; `cur` keeps pointing to the version).
//!
//! **Safety.** A list that failed to load or shrank (§16.5 `minListShare`, `minEntries`) is
//! not trusted: nothing changes in its division. A listed version whose jar could not be
//! fetched (or that waits for the next run) keeps the bot's current version in that division:
//! nothing is retired or delisted because its replacement is missing. More delistings than
//! `maxDelistPerRun` delist nothing ([`plan`] with `allow_delist` false).
//!
//! The Lambda reads the lists, `REG` and the bots, asks [`wanted`] which versions to fetch,
//! fetches and validates them, then [`plan`]s with the ones it has and [`apply`]s the plan
//! per bot.

use crate::bots::{imported_bot, imported_version};
use crate::config::SyncConfig;
use crate::ids::Division;
use crate::jar::JarInfo;
use crate::participants::{Entry, ParsedList};
use crate::store::{
    BotItem, BotSource, BotState, DAY, Store, StoreError, StoreResult, VersionItem, VersionState,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The sync's own state in the state bucket.
pub const SYNC_KEY: &str = "state/sync.json";

/// A failure is alerted once when it is this old.
pub const FAILURE_ALERT_SECONDS: u64 = 7 * DAY;

/// `state/sync.json`: the last good list sizes (the shrink guard) and the versions that could
/// not be fetched or validated. Times in unix seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncState {
    pub last_run: Option<u64>,
    /// By division (`1v1`, `melee`): the last trusted list.
    pub divisions: BTreeMap<String, DivisionSync>,
    /// By `"<id> <version>"`.
    pub failures: BTreeMap<String, Failure>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DivisionSync {
    /// Entries of the list.
    pub entries: usize,
    /// When it was read.
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Failure {
    pub first: u64,
    pub last: u64,
    pub error: String,
    /// The 7-day alert was raised.
    pub alerted: bool,
}

/// The key of a version in [`SyncState::failures`] and the fetched jars.
pub fn version_key(id: &str, version: &str) -> String {
    format!("{id} {version}")
}

/// Whether a list can be trusted this run: it has at least `minEntries` entries and at least
/// `minListShare` of the last trusted list's. `Err` says why not (logged as an alert).
pub fn check_list(
    list: &ParsedList,
    previous: Option<&DivisionSync>,
    config: &SyncConfig,
) -> Result<(), String> {
    let entries = list.entries.len();
    if entries < config.min_entries as usize {
        return Err(format!(
            "{entries} entries, fewer than {}",
            config.min_entries
        ));
    }
    if let Some(previous) = previous {
        let floor = previous.entries as f64 * config.min_list_share;
        if (entries as f64) < floor {
            return Err(format!(
                "{entries} entries, fewer than {:.0}% of the {} of the last sync",
                config.min_list_share * 100.0,
                previous.entries
            ));
        }
    }
    Ok(())
}

/// A list by bot id. A bot listed twice (another version) is its last line's version, which
/// is the one the seeding import made current.
pub type Listing = BTreeMap<String, Entry>;

pub fn listing(list: &ParsedList) -> Listing {
    let mut by_id = Listing::new();
    for entry in &list.entries {
        by_id.insert(entry.id.clone(), entry.clone());
    }
    by_id
}

/// What the sync looks at.
pub struct SyncInput<'a> {
    /// The trusted lists of this run; a division without one is left as it is.
    pub lists: BTreeMap<Division, &'a Listing>,
    pub bots: &'a BTreeMap<String, BotItem>,
    pub versions: &'a [VersionItem],
}

/// Whether the sync manages a bot: unclaimed, imported, not taken down.
pub fn managed(bot: &BotItem) -> bool {
    bot.source == BotSource::Import && bot.owner.is_none() && bot.state != BotState::Removed
}

/// A listed version to fetch: not registered yet, of a managed or an unknown bot.
#[derive(Clone, Debug, PartialEq)]
pub struct Wanted {
    pub entry: Entry,
    pub divisions: Vec<Division>,
    /// The bot is not known at all.
    pub new_bot: bool,
}

impl SyncInput<'_> {
    fn bot_versions(&self) -> HashMap<&str, Vec<&VersionItem>> {
        let mut by_bot: HashMap<&str, Vec<&VersionItem>> = HashMap::new();
        for v in self.versions {
            by_bot.entry(v.bot.as_str()).or_default().push(v);
        }
        by_bot
    }

    /// Every listed bot id (sorted), with its version per trusted division.
    fn listed(&self) -> BTreeMap<&str, BTreeMap<Division, &Entry>> {
        let mut listed: BTreeMap<&str, BTreeMap<Division, &Entry>> = BTreeMap::new();
        for (&division, listing) in &self.lists {
            for (id, entry) in listing.iter() {
                listed
                    .entry(id.as_str())
                    .or_default()
                    .insert(division, entry);
            }
        }
        listed
    }
}

/// Groups a bot's listing by version: each listed version with its divisions (in
/// `Division::ALL` order) and an entry (the first division's).
fn by_version<'a>(listed: &BTreeMap<Division, &'a Entry>) -> Vec<(&'a Entry, Vec<Division>)> {
    let mut out: Vec<(&Entry, Vec<Division>)> = Vec::new();
    for division in Division::ALL {
        let Some(&entry) = listed.get(&division) else {
            continue;
        };
        match out.iter_mut().find(|(e, _)| e.version == entry.version) {
            Some((_, divisions)) => divisions.push(division),
            None => out.push((entry, vec![division])),
        }
    }
    out
}

/// The listed versions to fetch (§16.5): those of unknown bots and new ones of managed bots,
/// by id and version.
pub fn wanted(input: &SyncInput) -> Vec<Wanted> {
    let versions = input.bot_versions();
    let mut out = Vec::new();
    for (id, listed) in input.listed() {
        let bot = input.bots.get(id);
        if bot.is_some_and(|b| !managed(b)) {
            continue;
        }
        let known = versions.get(id).map_or(&[][..], |v| &v[..]);
        for (entry, divisions) in by_version(&listed) {
            if !known.iter().any(|v| v.version == entry.version) {
                out.push(Wanted {
                    entry: entry.clone(),
                    divisions,
                    new_bot: bot.is_none(),
                });
            }
        }
    }
    out
}

/// What happens to an existing version.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// An active version's divisions change (it left a list or joined one).
    Divisions {
        vid: u32,
        from: Vec<Division>,
        to: Vec<Division>,
    },
    /// Delisted → active, with its pairings.
    Relist { vid: u32, divisions: Vec<Division> },
    /// Retired → active, from scratch.
    Reactivate { vid: u32, divisions: Vec<Division> },
    /// Replaced by another listed version of the bot.
    Retire { vid: u32 },
    /// The bot left every list.
    Delist { vid: u32 },
}

/// The bot's `cur` after the sync.
#[derive(Clone, Debug, PartialEq)]
pub enum Current {
    Keep,
    Vid(u32),
    /// The version of this name, imported in this run.
    Imported(String),
}

/// A new version to register, of a jar the Lambda has.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub entry: Entry,
    pub divisions: Vec<Division>,
}

/// The sync of one bot.
#[derive(Clone, Debug, PartialEq)]
pub struct BotPlan {
    pub id: String,
    /// The bot's `vs` the plan was made from (`None`: a new bot); it is applied only if they
    /// are unchanged.
    pub expected_versions: Option<Vec<u32>>,
    pub imports: Vec<Import>,
    pub changes: Vec<Change>,
    pub current: Current,
    pub divisions: Vec<Division>,
    pub state: BotState,
}

/// A listed bot the sync leaves alone, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    pub id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncPlan {
    pub bots: Vec<BotPlan>,
    pub skipped: Vec<Skipped>,
    /// Versions [`plan`] would delist (counted also when `allow_delist` is false).
    pub delistings: usize,
}

fn sorted(set: impl IntoIterator<Item = Division>) -> Vec<Division> {
    let set: BTreeSet<Division> = set.into_iter().collect();
    set.into_iter().collect()
}

/// The sync of every listed or managed bot (§16.5). `available`: the [`version_key`]s of the
/// wanted versions whose jars the Lambda has; the others keep the bot's current version in
/// their divisions. With `allow_delist` false, a bot that left every list stays as it is.
pub fn plan(input: &SyncInput, available: &HashSet<String>, allow_delist: bool) -> SyncPlan {
    let versions = input.bot_versions();
    let listed = input.listed();
    let trusted: BTreeSet<Division> = input.lists.keys().copied().collect();
    let mut out = SyncPlan::default();
    let mut ids: BTreeSet<&str> = listed.keys().copied().collect();
    ids.extend(
        input
            .bots
            .values()
            .filter(|b| managed(b))
            .map(|b| b.id.as_str()),
    );
    let none = BTreeMap::new();
    for id in ids {
        let listing = listed.get(id).unwrap_or(&none);
        let known = versions.get(id).map_or(&[][..], |v| &v[..]);
        let Some(bot) = input.bots.get(id) else {
            if let Some(p) = plan_new_bot(id, listing, available) {
                out.bots.push(p);
            }
            continue;
        };
        if !managed(bot) {
            let reason = if bot.state == BotState::Removed {
                "removed"
            } else if bot.owner.is_some() {
                "claimed"
            } else {
                "submitted"
            };
            out.skipped.push(Skipped {
                id: id.to_string(),
                reason: reason.to_string(),
            });
            continue;
        }
        let (p, delist) = plan_bot(bot, known, listing, &trusted, available, &mut out.skipped);
        out.delistings += delist;
        if let Some(p) = p
            && (allow_delist || delist == 0)
        {
            out.bots.push(p);
        }
    }
    out
}

fn plan_new_bot(
    id: &str,
    listing: &BTreeMap<Division, &Entry>,
    available: &HashSet<String>,
) -> Option<BotPlan> {
    let imports: Vec<Import> = by_version(listing)
        .into_iter()
        .filter(|(e, _)| available.contains(&version_key(&e.id, &e.version)))
        .map(|(e, divisions)| Import {
            entry: e.clone(),
            divisions,
        })
        .collect();
    let first = imports.first()?;
    Some(BotPlan {
        id: id.to_string(),
        expected_versions: None,
        current: Current::Imported(first.entry.version.clone()),
        divisions: sorted(imports.iter().flat_map(|i| i.divisions.iter().copied())),
        state: BotState::Active,
        imports,
        changes: Vec::new(),
    })
}

/// One managed bot: the plan if anything changes, and the versions it delists.
fn plan_bot(
    bot: &BotItem,
    known: &[&VersionItem],
    listing: &BTreeMap<Division, &Entry>,
    trusted: &BTreeSet<Division>,
    available: &HashSet<String>,
    skipped: &mut Vec<Skipped>,
) -> (Option<BotPlan>, usize) {
    // Divisions whose listed version can't be had this run: they keep what they have.
    let mut pending: BTreeSet<Division> = BTreeSet::new();
    let mut imports = Vec::new();
    for (entry, divisions) in by_version(listing) {
        match known.iter().find(|v| v.version == entry.version) {
            Some(v) if matches!(v.state, VersionState::Probation | VersionState::Failed) => {
                skipped.push(Skipped {
                    id: bot.id.clone(),
                    reason: format!("{} is {}", v.version, v.state.as_str()),
                });
                pending.extend(divisions);
            }
            Some(_) => {}
            None if available.contains(&version_key(&entry.id, &entry.version)) => {
                imports.push(Import {
                    entry: entry.clone(),
                    divisions,
                });
            }
            None => pending.extend(divisions),
        }
    }

    // Each existing version's divisions after the sync.
    let mut targets: Vec<(&VersionItem, BTreeSet<Division>)> = Vec::new();
    for &v in known {
        let mut target: BTreeSet<Division> = trusted
            .iter()
            .copied()
            .filter(|d| listing.get(d).is_some_and(|e| e.version == v.version))
            .collect();
        match v.state {
            VersionState::Active => {
                // Untrusted lists and missing replacements change nothing.
                target.extend(
                    v.divisions
                        .iter()
                        .filter(|d| !trusted.contains(d) || pending.contains(d)),
                );
            }
            VersionState::Delisted | VersionState::Retired => {}
            _ => target.clear(),
        }
        targets.push((v, target));
    }
    let listed_somewhere = !imports.is_empty() || targets.iter().any(|(_, t)| !t.is_empty());

    let mut changes = Vec::new();
    let mut delist = 0;
    for (v, target) in &targets {
        let to: Vec<Division> = target.iter().copied().collect();
        match v.state {
            VersionState::Active if !to.is_empty() => {
                if to != sorted(v.divisions.iter().copied()) {
                    changes.push(Change::Divisions {
                        vid: v.vid,
                        from: v.divisions.clone(),
                        to,
                    });
                }
            }
            VersionState::Active if listed_somewhere => changes.push(Change::Retire { vid: v.vid }),
            // Its replacement is missing: nothing changes.
            VersionState::Active if !pending.is_empty() => {}
            VersionState::Active => {
                changes.push(Change::Delist { vid: v.vid });
                delist += 1;
            }
            VersionState::Delisted if !to.is_empty() => changes.push(Change::Relist {
                vid: v.vid,
                divisions: to,
            }),
            VersionState::Delisted if listed_somewhere => {
                changes.push(Change::Retire { vid: v.vid })
            }
            VersionState::Retired if !to.is_empty() => changes.push(Change::Reactivate {
                vid: v.vid,
                divisions: to,
            }),
            _ => {}
        }
    }

    let active_after = |vid: u32| targets.iter().any(|(v, t)| v.vid == vid && !t.is_empty());
    let (current, divisions, state) = if listed_somewhere {
        // The version on the first list (1v1, then melee) is current.
        let primary = Division::ALL.iter().find_map(|d| {
            let entry = listing.get(d)?;
            if let Some((v, _)) = targets
                .iter()
                .find(|(v, t)| v.version == entry.version && !t.is_empty())
            {
                return Some(Current::Vid(v.vid));
            }
            imports
                .iter()
                .any(|i| i.entry.version == entry.version)
                .then(|| Current::Imported(entry.version.clone()))
        });
        let current = match primary {
            Some(Current::Vid(vid)) if Some(vid) == bot.current => Current::Keep,
            Some(primary) => primary,
            // Only untrusted or pending divisions hold active versions.
            None if bot.current.is_some_and(active_after) => Current::Keep,
            None => targets
                .iter()
                .find(|(_, t)| !t.is_empty())
                .map_or(Current::Keep, |(v, _)| Current::Vid(v.vid)),
        };
        let divisions = sorted(
            targets
                .iter()
                .flat_map(|(_, t)| t.iter().copied())
                .chain(imports.iter().flat_map(|i| i.divisions.iter().copied())),
        );
        (current, divisions, BotState::Active)
    } else if delist > 0 {
        (Current::Keep, bot.divisions.clone(), BotState::Delisted)
    } else {
        (Current::Keep, bot.divisions.clone(), bot.state)
    };
    let current = match current {
        Current::Vid(vid) if Some(vid) == bot.current => Current::Keep,
        other => other,
    };
    let unchanged = imports.is_empty()
        && changes.is_empty()
        && current == Current::Keep
        && divisions == bot.divisions
        && state == bot.state;
    if unchanged {
        return (None, delist);
    }
    (
        Some(BotPlan {
            id: bot.id.clone(),
            expected_versions: Some(bot.versions.clone()),
            imports,
            changes,
            current,
            divisions,
            state,
        }),
        delist,
    )
}

/// A jar the Lambda fetched and validated for a wanted version.
#[derive(Clone, Debug, PartialEq)]
pub struct FetchedJar {
    pub info: JarInfo,
    /// Where it came from (the version's `url`).
    pub url: String,
}

/// What applying one bot's plan did, for the report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncCounts {
    pub imported: usize,
    pub new_bots: usize,
    pub relisted: usize,
    pub reactivated: usize,
    pub retired: usize,
    pub delisted: usize,
    pub divisions_added: usize,
    pub divisions_removed: usize,
    /// Bots whose plan was not applied (changed meanwhile, or a write's condition failed).
    pub conflicts: usize,
}

impl SyncCounts {
    pub fn add(&mut self, other: &SyncCounts) {
        self.imported += other.imported;
        self.new_bots += other.new_bots;
        self.relisted += other.relisted;
        self.reactivated += other.reactivated;
        self.retired += other.retired;
        self.delisted += other.delisted;
        self.divisions_added += other.divisions_added;
        self.divisions_removed += other.divisions_removed;
        self.conflicts += other.conflicts;
    }
}

/// The result of applying a bot's plan: its counts, the vids reactivated from `retired` (the
/// `cleanup` job must forget it cleaned them), and lines to log.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub counts: SyncCounts,
    pub reactivated: Vec<u32>,
    pub log: Vec<String>,
}

/// Applies one bot's plan (§16.5): the new versions first (each registered with the bot in
/// one conditional transaction), then the existing versions (each conditional on the state the
/// plan saw), then the bot's `dv`, `st` and `cur`. A bot that changed since it was read (a
/// claim, a takedown, a registration) is left for the next run. `now_ms` in milliseconds:
/// new versions are released now.
pub fn apply<S: Store + ?Sized>(
    store: &S,
    plan: &BotPlan,
    jars: &HashMap<String, FetchedJar>,
    now_ms: u64,
) -> StoreResult<Applied> {
    let now = now_ms / 1000;
    let mut applied = Applied::default();
    let conflict = |mut applied: Applied, why: &str| -> StoreResult<Applied> {
        applied.counts.conflicts += 1;
        applied
            .log
            .push(format!("sync: {}: {why}; left for the next run", plan.id));
        Ok(applied)
    };
    let mut bot = store.get_bot(&plan.id)?;
    match (&bot, &plan.expected_versions) {
        (None, None) => {}
        (Some(b), Some(expected)) if managed(b) && &b.versions == expected => {}
        _ => return conflict(applied, "changed since it was read"),
    }

    let mut imported: HashMap<&str, u32> = HashMap::new();
    for import in &plan.imports {
        let key = version_key(&import.entry.id, &import.entry.version);
        let Some(jar) = jars.get(&key) else {
            return conflict(applied, &format!("no jar for {key}"));
        };
        let vid = store.next_vid()?;
        let version = imported_version(
            &jar.info,
            vid,
            import.divisions.clone(),
            VersionState::Active,
            Some(jar.url.clone()),
            Some(now_ms),
            now,
        );
        let expected = bot.as_ref().map(|b| b.versions.clone());
        let item = match &bot {
            None => imported_bot(&jar.info, None, import.divisions.clone(), vid, now),
            Some(b) => {
                let mut b = b.clone();
                b.versions.insert(0, vid);
                b
            }
        };
        match store.register_version(&item, expected.as_deref(), &version) {
            Ok(()) => {}
            Err(StoreError::ConditionFailed) => {
                return conflict(applied, "a concurrent registration");
            }
            Err(e) => return Err(e),
        }
        if bot.is_none() {
            applied.counts.new_bots += 1;
        }
        applied.counts.imported += 1;
        applied.log.push(format!(
            "sync: {} {} imported (vid {vid}, {})",
            plan.id,
            import.entry.version,
            division_names(&import.divisions)
        ));
        imported.insert(import.entry.version.as_str(), vid);
        bot = Some(item);
    }
    let Some(mut bot) = bot else {
        return Ok(applied);
    };

    for change in &plan.changes {
        let vid = match change {
            Change::Divisions { vid, .. }
            | Change::Relist { vid, .. }
            | Change::Reactivate { vid, .. }
            | Change::Retire { vid }
            | Change::Delist { vid } => *vid,
        };
        let Some(mut v) = store.get_version(vid)? else {
            continue;
        };
        let expected = v.state;
        let line = match change {
            Change::Divisions { from, to, .. } => {
                if expected != VersionState::Active {
                    continue;
                }
                let added = to.iter().filter(|d| !from.contains(d)).count();
                applied.counts.divisions_added += added;
                applied.counts.divisions_removed += from.iter().filter(|d| !to.contains(d)).count();
                v.divisions = to.clone();
                format!("divisions {}", division_names(to))
            }
            Change::Relist { divisions, .. } => {
                if expected != VersionState::Delisted {
                    continue;
                }
                applied.counts.relisted += 1;
                v.state = VersionState::Active;
                v.divisions = divisions.clone();
                format!("listed again ({})", division_names(divisions))
            }
            Change::Reactivate { divisions, .. } => {
                if expected != VersionState::Retired {
                    continue;
                }
                applied.counts.reactivated += 1;
                applied.reactivated.push(vid);
                v.state = VersionState::Active;
                v.retired = None;
                v.divisions = divisions.clone();
                format!("active again ({})", division_names(divisions))
            }
            Change::Retire { .. } => {
                if !matches!(expected, VersionState::Active | VersionState::Delisted) {
                    continue;
                }
                applied.counts.retired += 1;
                v.state = VersionState::Retired;
                v.retired = Some(now);
                "retired (replaced)".to_string()
            }
            Change::Delist { .. } => {
                if expected != VersionState::Active {
                    continue;
                }
                applied.counts.delisted += 1;
                v.state = VersionState::Delisted;
                "delisted".to_string()
            }
        };
        match store.update_version(&v, expected) {
            Ok(()) => applied
                .log
                .push(format!("sync: {} {} (vid {vid}) {line}", v.bot, v.version)),
            Err(StoreError::ConditionFailed) => {
                applied.counts.conflicts += 1;
            }
            Err(e) => return Err(e),
        }
    }

    match &plan.current {
        Current::Keep => {}
        Current::Vid(vid) => bot.current = Some(*vid),
        Current::Imported(version) => {
            if let Some(&vid) = imported.get(version.as_str()) {
                bot.current = Some(vid);
            }
        }
    }
    bot.divisions = plan.divisions.clone();
    bot.state = plan.state;
    store.put_bot(&bot)?;
    Ok(applied)
}

fn division_names(divisions: &[Division]) -> String {
    divisions
        .iter()
        .map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Notes this run's fetch failures (`"<id> <version>"` → error) in the state: a failure seen
/// again keeps its first time. `pending`: the wanted versions not fetched in this run (failed,
/// or left for the next run); failures of other versions (fetched now, or no longer listed)
/// go. Returns the `ALERT sync ...` lines of failures that turned 7 days old.
pub fn note_failures(
    state: &mut SyncState,
    failed: &BTreeMap<String, String>,
    pending: &HashSet<String>,
    now: u64,
) -> Vec<String> {
    state
        .failures
        .retain(|key, _| pending.contains(key) || failed.contains_key(key));
    for (key, error) in failed {
        let failure = state.failures.entry(key.clone()).or_insert(Failure {
            first: now,
            ..Failure::default()
        });
        failure.last = now;
        failure.error = error.clone();
    }
    let mut alerts = Vec::new();
    for (key, failure) in &mut state.failures {
        if !failure.alerted && now.saturating_sub(failure.first) >= FAILURE_ALERT_SECONDS {
            failure.alerted = true;
            alerts.push(format!(
                "ALERT sync {key} could not be fetched for 7 days: {}",
                failure.error
            ));
        }
    }
    alerts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WeightClass;
    use crate::participants::parse_list;
    use crate::store::MemStore;

    const NOW_MS: u64 = 1_792_843_200_000;
    const ONE: Division = Division::OneVsOne;
    const MELEE: Division = Division::Melee;

    fn version(
        vid: u32,
        bot: &str,
        name: &str,
        state: VersionState,
        dv: &[Division],
    ) -> VersionItem {
        VersionItem {
            vid,
            bot: bot.into(),
            version: name.into(),
            sha256: format!("{vid:064x}"),
            size: 100,
            codesize: Some(100),
            class: WeightClass::Mega,
            divisions: dv.to_vec(),
            state,
            probation_battles: 0,
            probation_failures: 0,
            class_name: bot.into(),
            created: 1,
            retired: None,
            url: None,
            colors: None,
            released: None,
        }
    }

    fn bot(id: &str, versions: &[u32], current: u32, dv: &[Division]) -> BotItem {
        BotItem {
            id: id.into(),
            author: "Ann".into(),
            owner: None,
            homepage: None,
            description: None,
            divisions: dv.to_vec(),
            versions: versions.to_vec(),
            current: Some(current),
            state: BotState::Active,
            created: 1,
            source: BotSource::Import,
        }
    }

    fn list(lines: &[&str]) -> Listing {
        let text: String = lines
            .iter()
            .map(|l| format!("{l},http://list/{}.jar\n", l.replace(' ', "_")))
            .collect();
        listing(&parse_list(&text))
    }

    struct World {
        bots: BTreeMap<String, BotItem>,
        versions: Vec<VersionItem>,
    }

    impl World {
        fn new(bots: Vec<BotItem>, versions: Vec<VersionItem>) -> World {
            World {
                bots: bots.into_iter().map(|b| (b.id.clone(), b)).collect(),
                versions,
            }
        }

        fn input<'a>(
            &'a self,
            one: Option<&'a Listing>,
            melee: Option<&'a Listing>,
        ) -> SyncInput<'a> {
            let mut lists = BTreeMap::new();
            if let Some(one) = one {
                lists.insert(ONE, one);
            }
            if let Some(melee) = melee {
                lists.insert(MELEE, melee);
            }
            SyncInput {
                lists,
                bots: &self.bots,
                versions: &self.versions,
            }
        }
    }

    fn all(wanted: &[Wanted]) -> HashSet<String> {
        wanted
            .iter()
            .map(|w| version_key(&w.entry.id, &w.entry.version))
            .collect()
    }

    fn only(plan: &SyncPlan, id: &str) -> BotPlan {
        let found: Vec<&BotPlan> = plan.bots.iter().filter(|p| p.id == id).collect();
        assert_eq!(found.len(), 1, "{id}: {:?}", plan.bots);
        found[0].clone()
    }

    #[test]
    fn new_bots_and_new_versions() {
        let world = World::new(
            vec![bot("a.A", &[1], 1, &[ONE, MELEE])],
            vec![version(1, "a.A", "1", VersionState::Active, &[ONE, MELEE])],
        );
        let one = list(&["a.A 2", "n.New 1.0"]);
        let melee = list(&["a.A 2", "n.New 1.0", "m.Melee 3"]);
        let input = world.input(Some(&one), Some(&melee));
        let wanted = wanted(&input);
        let keys: Vec<(String, Vec<Division>, bool)> = wanted
            .iter()
            .map(|w| {
                (
                    version_key(&w.entry.id, &w.entry.version),
                    w.divisions.clone(),
                    w.new_bot,
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("a.A 2".to_string(), vec![ONE, MELEE], false),
                ("m.Melee 3".to_string(), vec![MELEE], true),
                ("n.New 1.0".to_string(), vec![ONE, MELEE], true),
            ]
        );
        let plan = plan(&input, &all(&wanted), true);
        let a = only(&plan, "a.A");
        assert_eq!(a.imports.len(), 1);
        assert_eq!(a.changes, [Change::Retire { vid: 1 }]);
        assert_eq!(a.current, Current::Imported("2".into()));
        assert_eq!(a.divisions, [ONE, MELEE]);
        let n = only(&plan, "n.New");
        assert_eq!(
            (n.expected_versions.clone(), n.imports[0].divisions.clone()),
            (None, vec![ONE, MELEE])
        );
        assert_eq!(only(&plan, "m.Melee").divisions, [MELEE]);
        assert_eq!(plan.delistings, 0);

        // Without the new version's jar, the old one stays, and nothing happens to a.A.
        let plan = super::plan(&input, &HashSet::new(), true);
        assert!(plan.bots.is_empty(), "{:?}", plan.bots);
    }

    #[test]
    fn delist_relist_and_reactivate() {
        let world = World::new(
            vec![bot("a.A", &[2, 1], 2, &[ONE])],
            vec![
                version(1, "a.A", "1", VersionState::Retired, &[ONE]),
                version(2, "a.A", "2", VersionState::Active, &[ONE]),
            ],
        );
        let empty = list(&[]);
        let input = world.input(Some(&empty), Some(&empty));
        let plan = plan(&input, &HashSet::new(), true);
        let a = only(&plan, "a.A");
        assert_eq!(a.changes, [Change::Delist { vid: 2 }]);
        assert_eq!(
            (a.state, a.current.clone()),
            (BotState::Delisted, Current::Keep)
        );
        assert_eq!(plan.delistings, 1);
        // The guard: nothing is delisted.
        let guarded = super::plan(&input, &HashSet::new(), false);
        assert!(guarded.bots.is_empty());
        assert_eq!(guarded.delistings, 1);

        // Delisted, then the same version listed again: active, no new vid.
        let world = World::new(
            vec![BotItem {
                state: BotState::Delisted,
                ..bot("a.A", &[2, 1], 2, &[ONE])
            }],
            vec![
                version(1, "a.A", "1", VersionState::Retired, &[ONE]),
                version(2, "a.A", "2", VersionState::Delisted, &[ONE]),
            ],
        );
        let melee = list(&["a.A 2"]);
        let input = world.input(Some(&empty), Some(&melee));
        assert!(wanted(&input).is_empty());
        let a = only(&super::plan(&input, &HashSet::new(), true), "a.A");
        assert_eq!(
            a.changes,
            [Change::Relist {
                vid: 2,
                divisions: vec![MELEE]
            }]
        );
        assert_eq!(
            (a.state, a.divisions.clone()),
            (BotState::Active, vec![MELEE])
        );
        // Still delisted and listed nowhere: nothing to do.
        let input = world.input(Some(&empty), Some(&empty));
        assert!(super::plan(&input, &HashSet::new(), true).bots.is_empty());

        // The old, retired version listed: it is active again and current; the delisted one
        // retires.
        let old = list(&["a.A 1"]);
        let input = world.input(Some(&old), None);
        let a = only(&super::plan(&input, &HashSet::new(), true), "a.A");
        assert_eq!(
            a.changes,
            [
                Change::Reactivate {
                    vid: 1,
                    divisions: vec![ONE]
                },
                Change::Retire { vid: 2 }
            ]
        );
        assert_eq!(a.current, Current::Vid(1));
    }

    #[test]
    fn divisions_follow_the_lists() {
        let world = World::new(
            vec![
                bot("a.A", &[1], 1, &[ONE, MELEE]),
                bot("b.B", &[2], 2, &[ONE]),
            ],
            vec![
                version(1, "a.A", "1", VersionState::Active, &[ONE, MELEE]),
                version(2, "b.B", "1", VersionState::Active, &[ONE]),
            ],
        );
        // a.A left melee, b.B joined it.
        let one = list(&["a.A 1", "b.B 1"]);
        let melee = list(&["b.B 1"]);
        let plan = plan(
            &world.input(Some(&one), Some(&melee)),
            &HashSet::new(),
            true,
        );
        let a = only(&plan, "a.A");
        assert_eq!(
            a.changes,
            [Change::Divisions {
                vid: 1,
                from: vec![ONE, MELEE],
                to: vec![ONE]
            }]
        );
        assert_eq!(
            (a.divisions.clone(), a.current.clone()),
            (vec![ONE], Current::Keep)
        );
        let b = only(&plan, "b.B");
        assert_eq!(b.divisions, [ONE, MELEE]);

        // An untrusted melee list changes nothing in melee: a.A keeps it.
        let plan = super::plan(&world.input(Some(&one), None), &HashSet::new(), true);
        assert!(plan.bots.is_empty(), "{:?}", plan.bots);

        // Different versions on the two lists: each active in its own division; cur is the
        // 1v1 one.
        let one = list(&["a.A 1", "b.B 1"]);
        let melee = list(&["a.A 2", "b.B 1"]);
        let input = world.input(Some(&one), Some(&melee));
        let a = only(&super::plan(&input, &all(&wanted(&input)), true), "a.A");
        assert_eq!(a.imports[0].divisions, [MELEE]);
        assert_eq!(
            a.changes,
            [Change::Divisions {
                vid: 1,
                from: vec![ONE, MELEE],
                to: vec![ONE]
            }]
        );
        assert_eq!(a.current, Current::Keep);
    }

    #[test]
    fn a_missing_replacement_delists_nothing() {
        let world = World::new(
            vec![bot("a.A", &[1], 1, &[ONE])],
            vec![version(1, "a.A", "1", VersionState::Active, &[ONE])],
        );
        // a.A 2 is listed in melee only (and can't be had); a.A left 1v1. Without its
        // replacement, a.A 1 stays as it is.
        let one = list(&[]);
        let melee = list(&["a.A 2"]);
        let plan = plan(
            &world.input(Some(&one), Some(&melee)),
            &HashSet::new(),
            true,
        );
        assert!(plan.bots.is_empty(), "{:?}", plan.bots);
        assert_eq!(plan.delistings, 0);
    }

    #[test]
    fn owned_and_removed_bots_are_left_alone() {
        let claimed = BotItem {
            owner: Some("g1".into()),
            ..bot("c.C", &[1], 1, &[ONE])
        };
        let submitted = BotItem {
            source: BotSource::Submit,
            owner: Some("g2".into()),
            ..bot("s.S", &[2], 2, &[ONE])
        };
        let removed = BotItem {
            state: BotState::Removed,
            ..bot("r.R", &[3], 3, &[ONE])
        };
        let world = World::new(
            vec![claimed, submitted, removed],
            vec![
                version(1, "c.C", "1", VersionState::Active, &[ONE]),
                version(2, "s.S", "1", VersionState::Active, &[ONE]),
                version(3, "r.R", "1", VersionState::Removed, &[ONE]),
            ],
        );
        // Listed with new versions, or not at all: nothing to fetch, nothing to change.
        let one = list(&["c.C 2", "r.R 2"]);
        let input = world.input(Some(&one), Some(&one));
        assert!(wanted(&input).is_empty());
        let plan = plan(&input, &HashSet::new(), true);
        assert!(plan.bots.is_empty());
        let skipped: Vec<(&str, &str)> = plan
            .skipped
            .iter()
            .map(|s| (s.id.as_str(), s.reason.as_str()))
            .collect();
        assert_eq!(skipped, [("c.C", "claimed"), ("r.R", "removed")]);
    }

    fn parsed(n: usize) -> ParsedList {
        let text: String = (0..n).map(|i| format!("b.B{i} 1,http://x/{i}\n")).collect();
        parse_list(&text)
    }

    #[test]
    fn shrinking_lists_are_not_trusted() {
        let config = SyncConfig::default();
        assert!(check_list(&parsed(120), None, &config).is_ok());
        let err = check_list(&parsed(99), None, &config).unwrap_err();
        assert!(err.contains("fewer than 100"), "{err}");
        let previous = DivisionSync {
            entries: 1000,
            at: 0,
        };
        assert!(check_list(&parsed(900), Some(&previous), &config).is_ok());
        let err = check_list(&parsed(899), Some(&previous), &config).unwrap_err();
        assert!(err.contains("90% of the 1000"), "{err}");
    }

    #[test]
    fn the_last_line_of_a_bot_wins() {
        let l = list(&["a.A 1", "b.B 1", "a.A 2"]);
        assert_eq!(l["a.A"].version, "2");
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn failures_are_alerted_once_after_a_week() {
        let mut state = SyncState::default();
        let failed: BTreeMap<String, String> =
            [("a.A 2".to_string(), "download (HTTP 404)".to_string())].into();
        let wanted: HashSet<String> = ["a.A 2".to_string()].into();
        assert!(note_failures(&mut state, &failed, &wanted, 100).is_empty());
        assert!(note_failures(&mut state, &failed, &wanted, 100 + DAY).is_empty());
        assert_eq!(state.failures["a.A 2"].first, 100);
        let alerts = note_failures(&mut state, &failed, &wanted, 100 + 7 * DAY);
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].starts_with("ALERT sync a.A 2"), "{}", alerts[0]);
        assert!(note_failures(&mut state, &failed, &wanted, 100 + 8 * DAY).is_empty());
        // Fetched, or no longer listed: forgotten.
        assert!(note_failures(&mut state, &BTreeMap::new(), &HashSet::new(), 0).is_empty());
        assert!(state.failures.is_empty());
    }

    fn jar(id: &str, version: &str, sha: char) -> FetchedJar {
        FetchedJar {
            info: JarInfo {
                bot_id: id.into(),
                version: version.into(),
                main_class: id.into(),
                author: "Ann".into(),
                description: String::new(),
                webpage: String::new(),
                codesize: 600,
                weight_class: WeightClass::Micro,
                sha256: sha.to_string().repeat(64),
                size: 10,
            },
            url: format!("http://list/{id}_{version}.jar"),
        }
    }

    fn snapshot(store: &MemStore) -> World {
        let versions = store.list_versions().unwrap();
        let ids: BTreeSet<String> = versions.iter().map(|v| v.bot.clone()).collect();
        let bots = ids
            .iter()
            .filter_map(|id| store.get_bot(id).unwrap())
            .collect();
        World::new(bots, versions)
    }

    /// Plans and applies one run against the store, with every wanted jar available.
    fn run(store: &MemStore, one: &Listing, melee: &Listing, now_ms: u64) -> SyncCounts {
        let world = snapshot(store);
        let input = world.input(Some(one), Some(melee));
        let wanted = wanted(&input);
        let jars: HashMap<String, FetchedJar> = wanted
            .iter()
            .map(|w| {
                let key = version_key(&w.entry.id, &w.entry.version);
                let sha = char::from(b'a' + (key.len() % 6) as u8);
                (key, jar(&w.entry.id, &w.entry.version, sha))
            })
            .collect();
        let available: HashSet<String> = jars.keys().cloned().collect();
        let mut counts = SyncCounts::default();
        for p in plan(&input, &available, true).bots {
            counts.add(&apply(store, &p, &jars, now_ms).unwrap().counts);
        }
        counts
    }

    #[test]
    fn applying_runs_end_to_end() {
        let store = MemStore::new();
        // Day 1: two new bots, one in both lists.
        let counts = run(
            &store,
            &list(&["a.A 1", "b.B 1"]),
            &list(&["a.A 1"]),
            NOW_MS,
        );
        assert_eq!((counts.imported, counts.new_bots), (2, 2));
        let a = store.get_bot("a.A").unwrap().unwrap();
        assert_eq!(
            (a.divisions.clone(), a.current),
            (vec![ONE, MELEE], Some(1))
        );
        let v1 = store.get_version(1).unwrap().unwrap();
        assert_eq!(
            (v1.state, v1.released, v1.divisions.clone()),
            (VersionState::Active, Some(NOW_MS), vec![ONE, MELEE])
        );
        assert_eq!(v1.url.as_deref(), Some("http://list/a.A_1.jar"));

        // Day 2: the same lists change nothing.
        assert_eq!(
            run(
                &store,
                &list(&["a.A 1", "b.B 1"]),
                &list(&["a.A 1"]),
                NOW_MS + 1
            ),
            SyncCounts::default()
        );

        // Day 3: a.A has a new version; b.B left.
        let counts = run(&store, &list(&["a.A 2"]), &list(&["a.A 2"]), NOW_MS + 2);
        assert_eq!(
            (counts.imported, counts.retired, counts.delisted),
            (1, 1, 1)
        );
        let a = store.get_bot("a.A").unwrap().unwrap();
        assert_eq!((a.current, a.versions.clone()), (Some(3), vec![3, 1]));
        assert_eq!(
            store.get_version(1).unwrap().unwrap().state,
            VersionState::Retired
        );
        let b = store.get_bot("b.B").unwrap().unwrap();
        assert_eq!((b.state, b.current), (BotState::Delisted, Some(2)));
        assert_eq!(
            store.get_version(2).unwrap().unwrap().state,
            VersionState::Delisted
        );

        // Day 4: b.B is back with the same version; a.A's old version is listed in 1v1.
        let counts = run(
            &store,
            &list(&["a.A 1", "b.B 1"]),
            &list(&["a.A 2"]),
            NOW_MS + 3,
        );
        assert_eq!(
            (counts.relisted, counts.reactivated, counts.imported),
            (1, 1, 0)
        );
        let b = store.get_bot("b.B").unwrap().unwrap();
        assert_eq!(
            (b.state, b.current, b.versions.clone()),
            (BotState::Active, Some(2), vec![2])
        );
        let a = store.get_bot("a.A").unwrap().unwrap();
        assert_eq!(a.current, Some(1));
        assert_eq!(store.get_version(1).unwrap().unwrap().divisions, [ONE]);
        assert_eq!(store.get_version(3).unwrap().unwrap().divisions, [MELEE]);
    }

    #[test]
    fn a_bot_claimed_meanwhile_is_not_touched() {
        let store = MemStore::new();
        run(&store, &list(&["a.A 1"]), &list(&[]), NOW_MS);
        let world = snapshot(&store);
        let empty = list(&[]);
        let input = world.input(Some(&empty), Some(&empty));
        let p = only(&plan(&input, &HashSet::new(), true), "a.A");
        let mut claimed = store.get_bot("a.A").unwrap().unwrap();
        claimed.owner = Some("g1".into());
        store.put_bot(&claimed).unwrap();
        let applied = apply(&store, &p, &HashMap::new(), NOW_MS).unwrap();
        assert_eq!(applied.counts.conflicts, 1);
        assert_eq!(
            store.get_version(1).unwrap().unwrap().state,
            VersionState::Active
        );
    }
}
