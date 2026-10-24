// SPDX-License-Identifier: EPL-2.0

//! `publish` (spec §15): `plan` first, then per rumble the metrics from the division's state,
//! the data files in the site bucket (`data/v1/...`, `Cache-Control` of §14.6), the snapshots
//! in the state bucket, and one invalidation of `/data/v1/*`.
//!
//! Reads: `REG` (one Query), each bot's `M` item and the owners and contributors (GetItem
//! each, paced at [`READ_RATE`]). **Contributors without a users index:** the table has no
//! GSI, so the users are listed from `state/users.json`, the uids the planner saw in the log
//! (every user with battles has posted); each costs one eventually consistent GetItem
//! (0.5 RCU): a few hundred contributors are a few hundred RCU per run, 4 runs a day.
//!
//! Bot files are written only when their bytes changed: the GET that reads the old file's
//! history also returns its ETag, the MD5 of a single-part object, compared with the new
//! bytes' MD5.
//!
//! The recordings index (`data/v1/rec/`, [`super::recordings`]) runs after the rankings and
//! before the bot files, which carry its counts.

use super::plan::{self, PlanReport, USERS_KEY};
use super::{Batch, Pace, READ_RATE, json_bytes};
use crate::s3::{Bucket, Fetched, etag_matches};
use rumble_core::Config;
use rumble_core::batch::UserIndex;
use rumble_core::data::BotDetails;
use rumble_core::ids::RumbleId;
use rumble_core::publish::{
    Catalog, DATA_CACHE, DATA_PREFIX, RumbleRun, Snapshot, UserSnapshot, bot_details, bot_key,
    compute_rumble, contributors, date_of, expired_snapshots, index, rankings_key, shift_date,
    snapshot_key, user_snapshot,
};
use rumble_core::store::{BotItem, BotState, User, rfc3339};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const JSON: &str = "application/json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishReport {
    pub plan: PlanReport,
    /// Rumble id and members.
    pub rumbles: Vec<(String, usize)>,
    pub bot_files_written: usize,
    pub bot_files_unchanged: usize,
    pub bot_files_deleted: usize,
    /// Bots left for the next run (time budget).
    pub bot_files_skipped: usize,
    /// The recordings index (M6); `None` if it failed (the bot files keep their counts).
    pub recordings: Option<super::recordings::RecordingsReport>,
    pub contributors: usize,
    pub snapshots_deleted: usize,
}

fn put_data(batch: &Batch, key: &str, bytes: &[u8]) -> Result<(), String> {
    batch
        .objects()
        .put(Bucket::Site, key, bytes, JSON, Some(DATA_CACHE))
}

pub fn run(batch: &Batch, config: &Config) -> Result<PublishReport, String> {
    let planned = plan::run_states(batch, config)?;
    let start_ms = batch.now_ms();
    let engine = config.engine_version;
    let now = start_ms / 1000;
    let date = date_of(now);
    let generated_at = rfc3339(now);
    let store = batch.store();
    let mut report = PublishReport {
        plan: planned.report,
        ..PublishReport::default()
    };
    let mut pace = Pace::new(batch, READ_RATE);

    // The table: versions (after the plan's updates), bots, owners.
    let versions = store.list_versions().map_err(|e| e.to_string())?;
    let ids: BTreeSet<&str> = versions.iter().map(|v| v.bot.as_str()).collect();
    let mut bots: BTreeMap<String, BotItem> = BTreeMap::new();
    for id in ids {
        pace.take(0.5);
        if let Some(bot) = store.get_bot(id).map_err(|e| e.to_string())? {
            bots.insert(id.to_string(), bot);
        }
    }
    let mut owners: BTreeMap<String, User> = BTreeMap::new();
    for uid in bots.values().filter_map(|b| b.owner.clone()) {
        if owners.contains_key(&uid) {
            continue;
        }
        pace.take(0.5);
        if let Some(user) = store.get_user(&uid).map_err(|e| e.to_string())? {
            owners.insert(uid, user);
        }
    }
    let catalog = Catalog {
        versions: &versions,
        bots: &bots,
        owners: &owners,
    };

    // Rumbles and rankings.
    let week_ago = shift_date(&date, -7).unwrap_or_default();
    let mut runs: Vec<RumbleRun> = Vec::new();
    let mut summaries = Vec::new();
    for state in &planned.states {
        for id in RumbleId::of_division(state.division) {
            let previous: Option<Snapshot> = batch.read_json(
                Bucket::State,
                &snapshot_key(engine, &id.to_string(), &week_ago),
            )?;
            let run = compute_rumble(id, &catalog, state, previous.as_ref());
            if run.metrics.members.is_empty() {
                continue;
            }
            let rumble = id.to_string();
            put_data(
                batch,
                &rankings_key(&rumble),
                &json_bytes(&run.metrics.rankings(&rumble, &generated_at, engine)),
            )?;
            summaries.push(run.summary(config, state, engine));
            report.rumbles.push((rumble, run.metrics.members.len()));
            runs.push(run);
        }
    }
    put_data(
        batch,
        &format!("{DATA_PREFIX}/rumbles.json"),
        &json_bytes(&index(summaries, &generated_at)),
    )?;
    super::slot_pairs::write(batch, engine, &runs)?;

    // The recordings index (files under data/v1/rec/) and the counts the bot files carry. A
    // failure leaves the bot files' counts as they were.
    let counts = match super::recordings::run(batch, engine, &versions, &bots) {
        Ok((counts, recordings)) => {
            report.recordings = Some(recordings);
            Some(counts)
        }
        Err(e) => {
            println!("publish: recordings index failed: {e}");
            None
        }
    };

    // Bot files: written if changed; a removed bot's file goes.
    for (n, bot) in bots.values().enumerate() {
        if batch.now_ms().saturating_sub(start_ms) > batch.budget_ms {
            report.bot_files_skipped = bots.len() - n;
            println!(
                "publish: time budget used; {} bot files left for the next run",
                report.bot_files_skipped
            );
            break;
        }
        let key = bot_key(&bot.id);
        let (previous, etag) = match batch.objects().get(Bucket::Site, &key, None)? {
            Fetched::Found { bytes, etag } => (
                serde_json::from_slice::<BotDetails>(&bytes).ok(),
                Some(etag),
            ),
            _ => (None, None),
        };
        if bot.state == BotState::Removed {
            if etag.is_some() {
                batch.objects().delete(Bucket::Site, &key)?;
                report.bot_files_deleted += 1;
            }
            continue;
        }
        let Some(mut details) = bot_details(bot, &catalog, &runs, previous.as_ref(), &date) else {
            continue;
        };
        details.recordings = match &counts {
            Some(counts) => counts.get(&bot.id).cloned().unwrap_or_default(),
            None => previous
                .as_ref()
                .map(|p| p.recordings.clone())
                .unwrap_or_default(),
        };
        let bytes = json_bytes(&details);
        if etag.is_some_and(|e| etag_matches(&e, &bytes)) {
            report.bot_files_unchanged += 1;
        } else {
            put_data(batch, &key, &bytes)?;
            report.bot_files_written += 1;
        }
    }

    super::archive::write(batch, &versions, &bots, &generated_at)?;

    // Contributors, from the users the planner saw.
    let index_of_users: UserIndex = batch
        .read_json(Bucket::State, USERS_KEY)?
        .unwrap_or_default();
    let mut users = Vec::new();
    for uid in &index_of_users.uids {
        pace.take(0.5);
        if let Some(user) = store.get_user(uid).map_err(|e| e.to_string())? {
            users.push(user);
        }
    }
    let users_week: Option<UserSnapshot> =
        batch.read_json(Bucket::State, &snapshot_key(engine, "users", &week_ago))?;
    let list = contributors(&users, config, engine, &generated_at, users_week.as_ref());
    report.contributors = list.contributors.len();
    put_data(
        batch,
        &format!("{DATA_PREFIX}/contributors.json"),
        &json_bytes(&list),
    )?;

    // Today's snapshots (the day's last run wins), then the old ones go.
    for run in &runs {
        batch.write_state_json(
            &snapshot_key(engine, &run.id.to_string(), &date),
            &run.snapshot(),
        )?;
    }
    batch.write_state_json(
        &snapshot_key(engine, "users", &date),
        &user_snapshot(&users),
    )?;
    let keys = batch
        .objects()
        .list(Bucket::State, &format!("state/e{engine}/snapshots/"))?;
    for key in expired_snapshots(&keys, &date) {
        batch.objects().delete(Bucket::State, &key)?;
        report.snapshots_deleted += 1;
    }

    batch.services.cdn.invalidate(
        &[format!("/{DATA_PREFIX}/*")],
        &format!("publish-{start_ms}"),
    )?;
    Ok(report)
}
