// SPDX-License-Identifier: EPL-2.0

//! `sync` (daily, spec §16.5): the unclaimed imported bots follow the RoboWiki participants
//! lists, as in LiteRumble. Reads both lists (a list that fails or shrank is left out, with an
//! `ALERT sync`), `REG` and the bots (paced at [`READ_RATE`]); fetches the jars of new listed
//! versions (the list's URL, then the mirror; [`DOWNLOAD_GAP`] apart, at most
//! `sync.maxImportsPerRun`, within the time budget), validates them as an import does and puts
//! them in `jars/`; then plans and applies the changes with `rumble_core::sync`. Fetch
//! failures are kept in `state/sync.json`; one that is a week old raises an `ALERT sync` once.
//!
//! New versions and changed divisions reach the plan of the next hour (:12).

use super::{Batch, Pace, READ_RATE};
use crate::batch::cleanup::cleanup_key;
use crate::routes::jar_key;
use crate::s3::{Bucket, IMMUTABLE, JAR_TYPE};
use rumble_core::Config;
use rumble_core::cleanup::CleanupState;
use rumble_core::ids::Division;
use rumble_core::participants::{
    MAX_JAR_BYTES, MAX_LIST_BYTES, ParsedList, check_entry, mirror_url, parse_list,
};
use rumble_core::store::BotItem;
use rumble_core::sync::{
    self as core, FetchedJar, Listing, SYNC_KEY, Skipped, SyncCounts, SyncInput, SyncState,
    version_key,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;

/// The pause between two jar downloads (the archives are volunteers' servers).
pub const DOWNLOAD_GAP: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListReport {
    pub entries: usize,
    /// Used this run; otherwise `error` says why not.
    pub trusted: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub enabled: bool,
    pub lists: BTreeMap<String, ListReport>,
    /// New listed versions, fetched (jar valid), failed, and left for the next run.
    pub wanted: usize,
    pub fetched: usize,
    pub failed: usize,
    pub deferred: usize,
    #[serde(flatten)]
    pub counts: SyncCounts,
    /// Listed bots the sync doesn't manage (claimed, submitted, removed) or versions it
    /// can't take (probation, failed).
    pub skipped: Vec<Skipped>,
    /// More delistings than `sync.maxDelistPerRun`: none were made.
    pub delist_guard: bool,
    /// Bots whose plans were left for the next run (time budget).
    pub bots_left: usize,
    pub log: Vec<String>,
}

impl SyncReport {
    fn say(&mut self, line: String) {
        println!("{line}");
        self.log.push(line);
    }
}

fn fetch_list(batch: &Batch, url: &str) -> Result<ParsedList, String> {
    let bytes = batch.services.fetcher.fetch(url, MAX_LIST_BYTES)?;
    Ok(parse_list(&String::from_utf8_lossy(&bytes)))
}

/// The first source that yields a valid jar for `id version`: the list's URL, then the
/// mirror. The error is the most telling one, as in the importer: the first `invalid` or
/// `mismatch`, else the last download failure.
fn fetch_jar(
    batch: &Batch,
    config: &Config,
    id: &str,
    version: &str,
    url: &str,
) -> Result<FetchedJar, String> {
    let mut urls = vec![url.to_string()];
    if !config.sync.mirror.is_empty() {
        let mirror = mirror_url(&config.sync.mirror, id, version);
        if mirror != url {
            urls.push(mirror);
        }
    }
    let limit = MAX_JAR_BYTES.min(config.jar.max_bytes);
    let (mut invalid, mut download) = (None, None);
    for (n, url) in urls.into_iter().enumerate() {
        if n > 0 {
            (batch.services.sleep)(DOWNLOAD_GAP);
        }
        match batch.services.fetcher.fetch(&url, limit) {
            Ok(bytes) => match check_entry(&bytes, id, version, config) {
                Ok((info, _)) => {
                    let key = jar_key(&info.sha256);
                    let objects = batch.objects();
                    if !objects.exists(Bucket::Jars, &key)? {
                        objects.put(Bucket::Jars, &key, &bytes, JAR_TYPE, Some(IMMUTABLE))?;
                    }
                    return Ok(FetchedJar { info, url });
                }
                Err(why) => {
                    invalid.get_or_insert(why);
                }
            },
            Err(why) => download = Some(format!("download ({why})")),
        }
    }
    Err(invalid
        .or(download)
        .unwrap_or_else(|| "download (no source)".to_string()))
}

pub fn run(batch: &Batch, config: &Config) -> Result<SyncReport, String> {
    let start_ms = batch.now_ms();
    let now = start_ms / 1000;
    let over_budget = || batch.now_ms().saturating_sub(start_ms) > batch.budget_ms;
    let mut report = SyncReport {
        enabled: config.sync.enabled,
        ..SyncReport::default()
    };
    if !config.sync.enabled {
        return Ok(report);
    }
    let mut state: SyncState = batch
        .read_json(Bucket::State, SYNC_KEY)?
        .unwrap_or_default();

    // 1. The lists; an untrusted one changes nothing in its division.
    let mut listings: BTreeMap<Division, Listing> = BTreeMap::new();
    for division in Division::ALL {
        let name = division.as_str();
        let url = config.sync.lists.get(division);
        let checked = fetch_list(batch, url).and_then(|list| {
            core::check_list(&list, state.divisions.get(name), &config.sync)
                .map(|()| list)
                .map_err(|why| format!("{why} at {url}"))
        });
        match checked {
            Ok(list) => {
                report.lists.insert(
                    name.to_string(),
                    ListReport {
                        entries: list.entries.len(),
                        trusted: true,
                        error: None,
                    },
                );
                state.divisions.insert(
                    name.to_string(),
                    core::DivisionSync {
                        entries: list.entries.len(),
                        at: now,
                    },
                );
                listings.insert(division, core::listing(&list));
            }
            Err(why) => {
                report.say(format!("ALERT sync {name} list not used: {why}"));
                report.lists.insert(
                    name.to_string(),
                    ListReport {
                        entries: 0,
                        trusted: false,
                        error: Some(why),
                    },
                );
            }
        }
    }
    state.last_run = Some(now);
    if listings.is_empty() {
        batch.write_state_json(SYNC_KEY, &state)?;
        return Ok(report);
    }

    // 2. The registry and the bots.
    let store = batch.store();
    let mut pace = Pace::new(batch, READ_RATE);
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    // The registry's bots, and the listed ids it doesn't have (new bots, as far as `REG`
    // knows: one read each makes sure).
    let mut ids: BTreeSet<&str> = versions.iter().map(|v| v.bot.as_str()).collect();
    ids.extend(listings.values().flat_map(|l| l.keys().map(String::as_str)));
    let mut bots: BTreeMap<String, BotItem> = BTreeMap::new();
    for id in ids {
        pace.take(0.5);
        if let Some(bot) = store.get_bot(id).map_err(|e| e.to_string())? {
            bots.insert(id.to_string(), bot);
        }
    }
    let input = SyncInput {
        lists: listings.iter().map(|(&d, l)| (d, l)).collect(),
        bots: &bots,
        versions: &versions,
    };

    // 3. The jars of the new listed versions.
    let wanted = core::wanted(&input);
    report.wanted = wanted.len();
    let mut jars: HashMap<String, FetchedJar> = HashMap::new();
    let mut failed: BTreeMap<String, String> = BTreeMap::new();
    let mut pending: HashSet<String> = HashSet::new();
    let mut tried = 0u32;
    for w in &wanted {
        let key = version_key(&w.entry.id, &w.entry.version);
        if tried >= config.sync.max_imports_per_run || over_budget() {
            report.deferred += 1;
            pending.insert(key);
            continue;
        }
        if tried > 0 {
            (batch.services.sleep)(DOWNLOAD_GAP);
        }
        tried += 1;
        match fetch_jar(batch, config, &w.entry.id, &w.entry.version, &w.entry.url) {
            Ok(jar) => {
                jars.insert(key, jar);
            }
            Err(why) => {
                println!("sync: {key}: {why}");
                pending.insert(key.clone());
                failed.insert(key, why);
            }
        }
    }
    report.fetched = jars.len();
    report.failed = failed.len();
    for alert in core::note_failures(&mut state, &failed, &pending, now) {
        report.say(alert);
    }

    // 4. Plan (the delisting guard) and apply, bot by bot.
    let available: HashSet<String> = jars.keys().cloned().collect();
    let mut plan = core::plan(&input, &available, true);
    if plan.delistings > config.sync.max_delist_per_run as usize {
        report.delist_guard = true;
        report.say(format!(
            "ALERT sync {} versions would be delisted (more than {}); none are",
            plan.delistings, config.sync.max_delist_per_run
        ));
        plan = core::plan(&input, &available, false);
    }
    report.skipped = plan.skipped;
    let mut reactivated = Vec::new();
    for (n, bot) in plan.bots.iter().enumerate() {
        if over_budget() {
            report.bots_left = plan.bots.len() - n;
            report.say(format!(
                "sync: time budget used; {} bots left for the next run",
                report.bots_left
            ));
            break;
        }
        let applied = core::apply(store, bot, &jars, batch.now_ms()).map_err(|e| e.to_string())?;
        report.counts.add(&applied.counts);
        reactivated.extend(applied.reactivated);
        for line in applied.log {
            report.say(line);
        }
    }

    // 5. Versions active again were cleaned when they retired: `cleanup` must look again.
    if !reactivated.is_empty() {
        let key = cleanup_key(config.engine_version);
        let mut cleanup: CleanupState = batch.read_json(Bucket::State, &key)?.unwrap_or_default();
        let before = cleanup.cleaned.len();
        for vid in &reactivated {
            cleanup.cleaned.remove(vid);
        }
        if cleanup.cleaned.len() != before {
            batch.write_state_json(&key, &cleanup)?;
        }
    }
    batch.write_state_json(SYNC_KEY, &state)?;
    Ok(report)
}
