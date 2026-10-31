// SPDX-License-Identifier: EPL-2.0

//! The contributor loop (docs/rumble-backend.md §13.1 to §13.3, §13.5): asks the rumble for
//! batches of battles, runs them on parallel slots, posts the results, uploads the tracks the
//! server asks for and confirms them. Batches wait on disk (`<data>/rumble/pending/`) from
//! the moment their results are complete until the server has them, so a crash or a stop
//! during a network outage loses nothing.
//!
//! [`Contributor::new`] makes the start checks; [`Contributor::run`] is the loop, on the
//! caller's thread, with a [`Stop`] flag and a callback for the [`Event`]s a UI shows.

use crate::backoff::{self, Backoff};
use crate::battle::{Options, Outcome, Progress, Runner, check_engine, robot_name};
use crate::http::{Api, Error as ApiError};
use crate::jars::{JarCache, JarError};
use crate::tokenfile::now_iso;
use robocode_s_engine::ENGINE_VERSION;
use rumble_core::api::{
    BattleResult, BattleStatus, ClientInfo, Failure, FailureReason, Info, Me,
    RECORDINGS_DONE_MAX_SLOTS, RecordingsDoneRequest, ResultsRequest, ResultsResponse, Tier,
    WorkBattle, WorkBot, WorkQuery, WorkResponse,
};
use rumble_core::ids::Division;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The least time between two `GET work`s: the server's `rate.workSeconds` (10) and a second.
const WORK_INTERVAL: Duration = Duration::from_secs(11);
/// How much work one `GET work` asks for (§13.3).
pub const BATCH_WORK: Duration = Duration::from_secs(20 * 60);
/// Fewest battles asked for.
pub const MIN_BATCH: u32 = 4;
/// Battles the server hands out per batch at most, by default (§3 `batch.maxBattles`).
pub const MAX_BATCH_1V1: u32 = 50;
pub const MAX_BATCH_MELEE: u32 = 10;
/// Slots a melee battle takes (§13.2).
pub const MELEE_SLOTS: usize = 5;
/// Battles that end `error` in a row before the loop stops.
pub const MAX_ERRORS_IN_A_ROW: u32 = 3;
/// Unstarted battles become `notRun` this long before the batch's deadline (§13.3).
const CUT_BEFORE_DEADLINE: u64 = 10 * 60;
/// Running battles are aborted this long before it.
const ABORT_BEFORE_DEADLINE: u64 = 60;
/// Largest jar downloaded; the cache checks the announced size.
const JAR_LIMIT: u64 = 32 << 20;
/// `rumble/log.txt` keeps at most this much (§13.5).
pub const LOG_CAP: u64 = 1 << 20;
/// Attempts at a track upload and at `recordings/done`.
const UPLOAD_TRIES: u32 = 3;
/// Tick of the main loop.
const TICK: Duration = Duration::from_millis(100);

// ---- stopping ----

/// The stop flag: the first [`Stop::stop`] ends the loop gently (no new battles, running ones
/// finish, results are posted); the second aborts the running battles (they are `notRun`) and
/// gives up retrying posts (they stay in `pending/` for the next start). Clones share it.
#[derive(Clone, Debug, Default)]
pub struct Stop(Arc<AtomicU8>);

impl Stop {
    pub fn new() -> Stop {
        Stop::default()
    }

    /// One more stop request: gentle, then abort.
    pub fn stop(&self) {
        // One signal handler calls this; no race worth a compare-and-swap loop.
        let n = self.0.load(Ordering::SeqCst);
        self.0.store((n + 1).min(2), Ordering::SeqCst);
    }

    /// Abort at once.
    pub fn abort(&self) {
        self.0.store(2, Ordering::SeqCst);
    }

    pub fn requested(&self) -> bool {
        self.0.load(Ordering::SeqCst) >= 1
    }

    pub fn aborting(&self) -> bool {
        self.0.load(Ordering::SeqCst) >= 2
    }
}

// ---- why the loop can't go on ----

/// What ends the loop or keeps it from starting.
#[derive(Clone, Debug, PartialEq)]
pub enum Fatal {
    /// 401: the token isn't known (anymore).
    LoginNeeded,
    /// 403, with the server's words.
    Forbidden(String),
    /// 409 `engine_mismatch`, or an engine or deterministic rules that differ from the
    /// server's: an update is needed.
    Engine(String),
    /// 426, an API version this client doesn't speak, or a release below `minClient`; the
    /// text names the download page.
    ClientOutdated(String),
    /// The rumble can't be reached for the start checks, or answers nonsense.
    Server(String),
    /// The sandbox policy (§13.1) refuses.
    Sandbox(String),
    /// The robot host or the Java runtime isn't usable.
    Host(String),
    /// Nothing the client could offer: no division it has slots for.
    NoDivisions(String),
    /// [`MAX_ERRORS_IN_A_ROW`] battles ended `error`; the text is the last reason.
    TooManyErrors(String),
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fatal::LoginNeeded => write!(
                f,
                "the rumble doesn't know this token (401): run `robocode-s rumble login`"
            ),
            Fatal::Forbidden(m) => write!(f, "the rumble refuses this account: {m}"),
            Fatal::Engine(m) => write!(f, "{m}"),
            Fatal::ClientOutdated(m) => write!(f, "{m}"),
            Fatal::Server(m) => write!(f, "{m}"),
            Fatal::Sandbox(m) => write!(f, "{m}"),
            Fatal::Host(m) => write!(f, "{m}"),
            Fatal::NoDivisions(m) => write!(f, "{m}"),
            Fatal::TooManyErrors(m) => write!(
                f,
                "{MAX_ERRORS_IN_A_ROW} battles in a row failed on this machine, stopping: {m}"
            ),
        }
    }
}

impl std::error::Error for Fatal {}

/// The fatal reading of an API error, for the errors that no retry fixes.
fn fatal_of(e: &ApiError) -> Option<Fatal> {
    Some(match e {
        ApiError::Unauthorized(_) => Fatal::LoginNeeded,
        ApiError::Forbidden(m) => Fatal::Forbidden(m.clone()),
        ApiError::EngineMismatch(m) => Fatal::Engine(format!(
            "the rumble ranks another engine version than this robocode-s has ({m}): update \
             robocode-s"
        )),
        ApiError::ClientOutdated(m) => Fatal::ClientOutdated(format!(
            "this robocode-s is too old for the rumble; download the current one: {m}"
        )),
        _ => return None,
    })
}

// ---- events ----

/// What a UI shows (§13.3). Serializes as one JSON object with an `event` field, for
/// `--json`. `batch` is the server's batch id.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// A batch arrived (jars are in the cache).
    Batch {
        batch: String,
        division: Division,
        battles: usize,
        deadline: String,
    },
    BattleStarted {
        batch: String,
        index: u32,
        division: Division,
        bots: Vec<String>,
    },
    /// Round `n` of `of` is over.
    Round {
        batch: String,
        index: u32,
        n: u32,
        of: u32,
    },
    /// A battle ended: `scores` and `ranks` are the robots' (in `bots` order) when it was
    /// `ok`; `detail` says why for the others.
    BattleDone {
        batch: String,
        index: u32,
        division: Division,
        bots: Vec<String>,
        status: BattleStatus,
        scores: Vec<f64>,
        ranks: Vec<u32>,
        duration_ms: Option<u64>,
        detail: Option<String>,
    },
    /// The server took a batch's results.
    Posted {
        batch: String,
        accepted: usize,
        rejected: usize,
        duplicate: bool,
        battles: u64,
        reputation: i64,
        tier: Tier,
    },
    /// `n` recordings uploaded and confirmed.
    Uploaded {
        batch: String,
        n: usize,
    },
    /// Nothing to do for `seconds`.
    Waiting {
        seconds: u64,
        why: String,
    },
    Error {
        text: String,
    },
}

impl Event {
    /// The line of `rumble/log.txt`, or nothing for the chatty events.
    fn log_line(&self) -> Option<String> {
        Some(match self {
            Event::Batch {
                batch,
                division,
                battles,
                deadline,
            } => format!("batch {batch}: {battles} {division} battles, deadline {deadline}"),
            Event::BattleDone {
                batch,
                index,
                status,
                duration_ms,
                detail,
                ..
            } => format!(
                "battle {batch}/{index}: {status:?}{}{}",
                duration_ms.map_or(String::new(), |ms| format!(" in {ms} ms")),
                detail.as_ref().map_or(String::new(), |d| format!(" ({d})"))
            ),
            Event::Posted {
                batch,
                accepted,
                rejected,
                duplicate,
                ..
            } => format!(
                "posted {batch}: {accepted} accepted, {rejected} rejected{}",
                if *duplicate { " (duplicate)" } else { "" }
            ),
            Event::Uploaded { batch, n } => format!("uploaded {n} recordings of {batch}"),
            Event::Waiting { seconds, why } => format!("waiting {seconds} s: {why}"),
            Event::Error { text } => format!("error: {text}"),
            Event::BattleStarted { .. } | Event::Round { .. } => return None,
        })
    }
}

/// What a run came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub battles_ok: u32,
    pub robot_failed: u32,
    pub not_run: u32,
    pub errors: u32,
    /// Batches the server took (pending ones from earlier runs included).
    pub batches_posted: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub uploaded: u32,
    /// What ended the run, if not a stop request or `max_batches`.
    pub fatal: Option<Fatal>,
}

// ---- configuration ----

/// How to contribute.
#[derive(Debug, Clone)]
pub struct Config {
    /// `https://<domain>`; the API is at `/api/v1/`.
    pub server: String,
    /// `rsu_...`
    pub token: String,
    /// What the user offers (melee only with [`MELEE_SLOTS`] slots).
    pub divisions: Vec<Division>,
    /// How battles run; `options.slots` is the number of parallel slots.
    pub options: Options,
    /// Stop after fetching this many batches (and posting them): `--batches N`.
    pub max_batches: Option<u32>,
    pub cache_max_bytes: u64,
}

impl Config {
    pub fn new(server: &str, token: &str, options: Options) -> Config {
        Config {
            server: server.trim_end_matches('/').to_string(),
            token: token.to_string(),
            divisions: Division::ALL.to_vec(),
            options,
            max_batches: None,
            cache_max_bytes: crate::jars::DEFAULT_MAX_BYTES,
        }
    }
}

/// `max(1, min(cores / 2, RAM GiB / 2))` (§13.2); without a way to ask for the RAM, the cores.
pub fn default_slots() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut slots = cores / 2;
    if let Some(gib) = total_memory_gib() {
        slots = slots.min((gib / 2) as usize);
    }
    slots.max(1)
}

#[cfg(target_os = "linux")]
fn total_memory_gib() -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = text
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb / (1 << 20))
}

#[cfg(not(target_os = "linux"))]
fn total_memory_gib() -> Option<u64> {
    None
}

/// Slots a battle of the division takes.
pub fn slot_cost(division: Division) -> usize {
    match division {
        Division::Melee => MELEE_SLOTS,
        Division::OneVsOne => 1,
    }
}

/// The divisions offered to the server: the wanted ones that fit in `slots`.
pub fn offered_divisions(wanted: &[Division], slots: usize) -> Vec<Division> {
    Division::ALL
        .into_iter()
        .filter(|d| wanted.contains(d) && slot_cost(*d) <= slots)
        .collect()
}

/// `max` of a work request (§13.3): the battles that fill about 20 minutes on `slots` slots,
/// judged by the average slot time per battle so far; [`MIN_BATCH`] while there is no
/// measurement; at most `cap`.
pub fn batch_size(avg_slot_ms: Option<f64>, slots: usize, cap: u32) -> u32 {
    let cap = cap.max(MIN_BATCH);
    let Some(avg) = avg_slot_ms.filter(|a| a.is_finite() && *a > 0.0) else {
        return MIN_BATCH;
    };
    let n = BATCH_WORK.as_millis() as f64 * slots as f64 / avg;
    (n.round().clamp(f64::from(MIN_BATCH), f64::from(cap))) as u32
}

/// Whether `release` (`0.2.0`) is at least `min` (`0.1.5`): numeric parts compared in order;
/// anything after a `-` is ignored.
pub fn release_at_least(release: &str, min: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect()
    }
    let (a, b) = (parts(release), parts(min));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    true
}

// ---- time ----

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Parses `2026-10-24T15:00:00Z` (UTC, optional fraction) into Unix seconds.
pub fn parse_iso(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let time = time.split('.').next()?;
    let mut t = time.split(':');
    let (h, min, s): (u64, u64, u64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || min > 59 || s > 60 {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant).
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days)
        .ok()
        .map(|days| days * 86_400 + h * 3600 + min * 60 + s)
}

// ---- the log ----

/// `rumble/log.txt`: lines of `time text`, trimmed from the front above [`LOG_CAP`].
#[derive(Debug)]
pub struct Log {
    path: PathBuf,
    size: u64,
}

impl Log {
    pub fn open(dir: &Path) -> Log {
        let _ = fs::create_dir_all(dir);
        let path = dir.join("log.txt");
        let size = fs::metadata(&path).map_or(0, |m| m.len());
        Log { path, size }
    }

    /// Appends a line; failures don't matter.
    pub fn line(&mut self, text: &str) {
        let text = text.replace('\n', " ");
        let line = format!("{} {text}\n", now_iso());
        if self.size + line.len() as u64 > LOG_CAP {
            self.trim();
        }
        let result = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if result.is_ok() {
            self.size += line.len() as u64;
        }
    }

    /// Keeps the last half of the cap, from a line start.
    fn trim(&mut self) {
        let Ok(bytes) = fs::read(&self.path) else {
            return;
        };
        let keep = (LOG_CAP / 2) as usize;
        let from = bytes.len().saturating_sub(keep);
        let from = bytes[from..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| from + i + 1);
        if fs::write(&self.path, &bytes[from..]).is_ok() {
            self.size = (bytes.len() - from) as u64;
        }
    }
}

// ---- pending batches (§13.5) ----

/// A batch whose results wait for the server: `pending/<batchId>.json`, with the tracks as
/// `pending/<batchId>/<index>.<round>.track`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingBatch {
    pub batch_id: String,
    /// The batch's deadline (ISO 8601): posting is given up after it.
    pub deadline: String,
    /// The `POST results` body.
    pub request: ResultsRequest,
}

fn file_stem(batch_id: &str) -> String {
    batch_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The pending files under `<data>/rumble/pending`.
#[derive(Clone, Debug)]
pub struct PendingDir(PathBuf);

impl PendingDir {
    pub fn in_data_dir(data: &Path) -> PendingDir {
        PendingDir(data.join("rumble").join("pending"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    fn json(&self, batch_id: &str) -> PathBuf {
        self.0.join(format!("{}.json", file_stem(batch_id)))
    }

    fn tracks(&self, batch_id: &str) -> PathBuf {
        self.0.join(file_stem(batch_id))
    }

    fn track(&self, batch_id: &str, index: u32, round: u32) -> PathBuf {
        self.tracks(batch_id).join(format!("{index}.{round}.track"))
    }

    /// Writes the tracks, then the request (a batch exists once its json does).
    pub fn write(&self, batch: &PendingBatch, tracks: &[Track]) -> io::Result<()> {
        fs::create_dir_all(&self.0)?;
        if !tracks.is_empty() {
            fs::create_dir_all(self.tracks(&batch.batch_id))?;
        }
        for t in tracks {
            fs::write(self.track(&batch.batch_id, t.index, t.round), &t.bytes)?;
        }
        let json = self.json(&batch.batch_id);
        let tmp = self.0.join(format!(
            ".{}.{}.tmp",
            file_stem(&batch.batch_id),
            std::process::id()
        ));
        fs::write(&tmp, serde_json::to_vec(batch).map_err(io::Error::other)?)?;
        fs::rename(&tmp, json)
    }

    /// The pending batches, oldest name first; unreadable files are removed. Track folders
    /// without a request are removed too.
    pub fn list(&self) -> Vec<PendingBatch> {
        let Ok(entries) = fs::read_dir(&self.0) else {
            return Vec::new();
        };
        let mut found: Vec<PendingBatch> = Vec::new();
        let mut folders = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                folders.push(path);
            } else if name.ends_with(".json") {
                match fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<PendingBatch>(&b).ok())
                {
                    Some(batch) => found.push(batch),
                    None => {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
        for folder in folders {
            let has_request = found.iter().any(|b| self.tracks(&b.batch_id) == folder);
            if !has_request {
                let _ = fs::remove_dir_all(folder);
            }
        }
        found.sort_by(|a, b| a.batch_id.cmp(&b.batch_id));
        found
    }

    pub fn read_track(&self, batch_id: &str, index: u32, round: u32) -> io::Result<Vec<u8>> {
        fs::read(self.track(batch_id, index, round))
    }

    /// Removes the batch's files.
    pub fn remove(&self, batch_id: &str) {
        let _ = fs::remove_file(self.json(batch_id));
        let _ = fs::remove_dir_all(self.tracks(batch_id));
    }
}

/// One round's track of a battle.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub index: u32,
    pub round: u32,
    pub bytes: Vec<u8>,
}

// ---- messages between the loop and its threads ----

enum Msg {
    Event(Event),
    Progress {
        key: u64,
        pos: usize,
        n: u32,
        of: u32,
    },
    Battle {
        key: u64,
        pos: usize,
        outcome: Box<Outcome>,
    },
    Fetched(Fetched),
    Posted(PostEnd),
}

enum Fetched {
    Batch(Box<Prepared>),
    /// Try again later: the server's `Retry-After` if it gave one, else the network backoff.
    Retry {
        after: Option<Duration>,
        why: String,
    },
    Fatal(Fatal),
}

/// A batch with its jars in the cache.
struct Prepared {
    work: WorkResponse,
    /// Per battle: the jar paths, or the result that stands for it without running.
    battles: Vec<Result<Vec<PathBuf>, Box<BattleResult>>>,
    /// Why premade results are what they are, by battle position (for events).
    notes: HashMap<usize, String>,
    /// A jar couldn't be fetched at all (network): back off before the next request.
    download_trouble: bool,
}

enum PostEnd {
    /// The server took it.
    Posted {
        accepted: u32,
        rejected: u32,
        uploaded: u32,
    },
    /// Given up for good (expired, refused as invalid): files removed.
    Dropped,
    /// Left in `pending/` for the next start.
    Kept,
    Fatal(Fatal),
}

// ---- the contributor ----

/// A checked, ready contributor.
pub struct Contributor {
    config: Config,
    api: Api,
    info: Info,
    me: Me,
    runner: Runner,
    cache: JarCache,
    pending: PendingDir,
    dir: PathBuf,
    platform: String,
    slots: usize,
    offered: Vec<Division>,
    /// When `GET work` was last asked: the server takes one a token every 10 s (spec §5.3),
    /// so a prefetch right after a batch would only be refused.
    last_work: std::sync::Mutex<Option<Instant>>,
}

impl fmt::Debug for Contributor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Contributor")
            .field("server", &self.config.server)
            .field("slots", &self.slots)
            .field("offered", &self.offered)
            .finish()
    }
}

impl Contributor {
    /// The start checks (§13.1) except reading the token: `GET info` (API major, `minClient`,
    /// engine and deterministic rules), the sandbox policy, the token (`GET me`) and the robot
    /// host. Posting the pending batches is the first thing [`Contributor::run`] does.
    pub fn new(config: Config) -> Result<Contributor, Fatal> {
        let api = Api::new(
            &format!("{}/api/v1", config.server.trim_end_matches('/')),
            Some(config.token.clone()),
            ENGINE_VERSION,
        );
        let info = api.info().map_err(|e| {
            fatal_of(&e).unwrap_or_else(|| {
                Fatal::Server(format!("can't ask the rumble at {}: {e}", config.server))
            })
        })?;
        if info.api != 1 {
            return Err(Fatal::ClientOutdated(format!(
                "the rumble speaks API version {}, this robocode-s speaks 1: download a newer \
                 robocode-s from {}",
                info.api, info.downloads
            )));
        }
        let release = env!("CARGO_PKG_VERSION");
        if !release_at_least(release, &info.min_client) {
            return Err(Fatal::ClientOutdated(format!(
                "robocode-s {release} is older than the {} the rumble needs: download the \
                 current one from {}",
                info.min_client, info.downloads
            )));
        }
        check_engine(&info).map_err(Fatal::Engine)?;
        let slots = config.options.slots.max(1);
        let offered = offered_divisions(&config.divisions, slots);
        if offered.is_empty() {
            return Err(Fatal::NoDivisions(format!(
                "no division to offer: melee battles need {MELEE_SLOTS} parallel slots (this \
                 has {slots}); add slots with --parallel, or choose 1v1"
            )));
        }
        // Rumble battles run on the pinned JRE (a container brings its own, the same release).
        if let Some(java) = &config.options.env.java
            && !config.options.env.sandbox.is_container()
        {
            robocode_s_runner::jre::check_pinned(java).map_err(Fatal::Host)?;
        }
        let runner = Runner::new(config.options.clone()).map_err(Fatal::Sandbox)?;
        let me = api.me().map_err(|e| {
            fatal_of(&e)
                .unwrap_or_else(|| Fatal::Server(format!("asking the rumble who you are: {e}")))
        })?;
        let platform = runner.check_host().map_err(Fatal::Host)?;
        let data = config.options.env.data.clone();
        let cache = JarCache::in_data_dir(&data).with_max_bytes(config.cache_max_bytes);
        let _ = cache.evict();
        Ok(Contributor {
            api,
            info,
            me,
            runner,
            cache,
            pending: PendingDir::in_data_dir(&data),
            dir: data.join("rumble"),
            platform,
            slots,
            offered,
            last_work: std::sync::Mutex::new(None),
            config,
        })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// The account, as of the start.
    pub fn me(&self) -> &Me {
        &self.me
    }

    /// The platform robots run on (`JVM 21.0.4`).
    pub fn platform(&self) -> &str {
        &self.platform
    }

    pub fn slots(&self) -> usize {
        self.slots
    }

    /// The divisions asked for (the wanted ones that fit in the slots).
    pub fn divisions(&self) -> &[Division] {
        &self.offered
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    pub fn pending(&self) -> &PendingDir {
        &self.pending
    }

    /// The loop (§13.3), on this thread, until `stop`, the batch limit or a fatal error.
    /// `on_event` is called on this thread. Pending batches from earlier runs are posted first.
    pub fn run(&self, stop: &Stop, mut on_event: impl FnMut(Event)) -> Summary {
        let mut log = Log::open(&self.dir);
        let mut emit = |event: Event| {
            if let Some(line) = event.log_line() {
                log.line(&line);
            }
            on_event(event);
        };
        let mut summary = Summary::default();
        for pending in self.pending.list() {
            if stop.requested() {
                return summary;
            }
            let end = self.post(&pending, &[], stop, &mut emit);
            if account(&mut summary, end) {
                return summary;
            }
        }
        self.run_loop(stop, &mut emit, &mut summary);
        summary
    }

    // ---- fetching a batch ----

    fn max_battles(&self, avg_slot_ms: Option<f64>) -> u32 {
        let cap = self
            .offered
            .iter()
            .map(|d| match d {
                Division::OneVsOne => MAX_BATCH_1V1,
                Division::Melee => MAX_BATCH_MELEE,
            })
            .max()
            .unwrap_or(MIN_BATCH);
        batch_size(avg_slot_ms, self.slots, cap)
    }

    /// `GET work` and the jars of what comes.
    fn fetch(&self, max: u32) -> Fetched {
        {
            let mut last = self.last_work.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(at) = *last {
                std::thread::sleep(WORK_INTERVAL.saturating_sub(at.elapsed()));
            }
            *last = Some(Instant::now());
        }
        let query = WorkQuery {
            engine: ENGINE_VERSION,
            max,
            divisions: self.offered.clone(),
            slots: self.slots as u32,
        };
        match self.api.work(&query) {
            Ok(crate::http::Work::Wait(after)) => Fetched::Retry {
                after: Some(after.unwrap_or(Duration::from_secs(30))),
                why: "no work for now".into(),
            },
            Ok(crate::http::Work::Batch(work)) => {
                if work.engine_version != ENGINE_VERSION {
                    return Fetched::Fatal(Fatal::Engine(format!(
                        "the rumble handed out a batch of engine version {}, this robocode-s \
                         has {ENGINE_VERSION}: update robocode-s",
                        work.engine_version
                    )));
                }
                Fetched::Batch(Box::new(self.prepare(work)))
            }
            Err(e) => match fatal_of(&e) {
                Some(f) => Fetched::Fatal(f),
                None if e.is_retryable() => Fetched::Retry {
                    after: e.retry_after(),
                    why: format!("asking for work: {e}"),
                },
                None => Fetched::Retry {
                    after: None,
                    why: format!("asking for work: {e}"),
                },
            },
        }
    }

    /// Downloads the batch's jars (four at a time) and decides which battles can run.
    fn prepare(&self, work: WorkResponse) -> Prepared {
        let mut bots: Vec<WorkBot> = Vec::new();
        for b in work.battles.iter().flat_map(|b| &b.bots) {
            if !bots.contains(b) {
                bots.push(b.clone());
            }
        }
        let fetch = |bot: &WorkBot| {
            self.api
                .download(&bot.url, JAR_LIMIT)
                .map_err(|e| e.to_string())
        };
        let results = self.cache.ensure(&bots, &fetch);
        let key = |b: &WorkBot| format!("{}\0{}\0{}", b.id, b.version, b.sha256);
        let jars: HashMap<String, Result<PathBuf, JarError>> =
            bots.iter().zip(results).map(|(b, r)| (key(b), r)).collect();
        let mut download_trouble = false;
        let mut notes = HashMap::new();
        let battles = work
            .battles
            .iter()
            .enumerate()
            .map(|(pos, battle)| {
                let mut paths = Vec::new();
                for (i, bot) in battle.bots.iter().enumerate() {
                    match &jars[&key(bot)] {
                        Ok(p) => paths.push(p.clone()),
                        Err(e) => {
                            let (result, note) = match e {
                                JarError::Mismatch(_) | JarError::BadName(_) => (
                                    robot_failed(battle.index, i, &format!("jar hash: {e}")),
                                    format!("{} {e}", robot_name(bot)),
                                ),
                                JarError::Fetch(_) => {
                                    download_trouble = true;
                                    (
                                        status_only(battle.index, BattleStatus::NotRun),
                                        format!("{} {e}", robot_name(bot)),
                                    )
                                }
                                JarError::Io(_) => (
                                    status_only(battle.index, BattleStatus::Error),
                                    format!("{} {e}", robot_name(bot)),
                                ),
                            };
                            notes.insert(pos, note);
                            return Err(Box::new(result));
                        }
                    }
                }
                Ok(paths)
            })
            .collect();
        Prepared {
            work,
            battles,
            notes,
            download_trouble,
        }
    }

    // ---- posting ----

    /// Posts a batch (§13.3): the request is on disk first, then retried until the server
    /// takes it or the deadline passes; then the requested tracks go up and are confirmed;
    /// then the files go. `tracks` are in memory for a fresh batch, empty for one read back
    /// from `pending/`.
    fn post(
        &self,
        batch: &PendingBatch,
        tracks: &[Track],
        stop: &Stop,
        emit: &mut dyn FnMut(Event),
    ) -> PostEnd {
        let id = &batch.batch_id;
        if !tracks.is_empty() || !self.pending.json(id).exists() {
            if let Err(e) = self.pending.write(batch, tracks) {
                emit(Event::Error {
                    text: format!("can't write the pending batch {id}: {e}"),
                });
            }
        }
        let deadline = parse_iso(&batch.deadline).unwrap_or_else(|| unix_now() + 3 * 3600);
        let mut backoff = Backoff::results();
        let response: ResultsResponse = loop {
            let error = match self.api.results(&batch.request) {
                Ok(r) => break r,
                Err(e) => e,
            };
            if let Some(f) = fatal_of(&error) {
                // Results of another engine are never counted: nothing to keep.
                if matches!(error, ApiError::EngineMismatch(_)) {
                    self.pending.remove(id);
                }
                return PostEnd::Fatal(f);
            }
            if !error.is_retryable() {
                let why = match error {
                    ApiError::Expired(_) => format!("batch {id} expired, its results are dropped"),
                    _ => format!("the rumble refused the results of batch {id}: {error}"),
                };
                emit(Event::Error { text: why });
                self.pending.remove(id);
                return PostEnd::Dropped;
            }
            let wait = match error.retry_after() {
                Some(after) => backoff::retry_after(after, backoff::random_unit()),
                None => backoff.next(backoff::random_unit()),
            };
            if unix_now() + wait.as_secs() >= deadline {
                emit(Event::Error {
                    text: format!("batch {id}: deadline passed before the results got through"),
                });
                self.pending.remove(id);
                return PostEnd::Dropped;
            }
            emit(Event::Waiting {
                seconds: wait.as_secs(),
                why: format!("posting the results of {id}: {error}"),
            });
            if stop.aborting() || !sleep(wait, stop) {
                return PostEnd::Kept;
            }
        };
        emit(Event::Posted {
            batch: id.clone(),
            accepted: response.accepted.len(),
            rejected: response.rejected.len(),
            duplicate: response.duplicate,
            battles: response.user.battles,
            reputation: response.user.reputation,
            tier: response.user.tier,
        });
        // A rejected battle is a bug on one side or the other: say which and why.
        for r in &response.rejected {
            emit(Event::Error {
                text: format!(
                    "battle {} of {id} rejected: {}: {}",
                    r.index, r.error, r.message
                ),
            });
        }

        // The tracks the server asked for.
        let mut uploaded: Vec<String> = Vec::new();
        for slot in &response.uploads {
            let bytes = match tracks
                .iter()
                .find(|t| t.index == slot.index && t.round == slot.round)
            {
                Some(t) => Ok(t.bytes.clone()),
                None => self.pending.read_track(id, slot.index, slot.round),
            };
            let bytes = match bytes {
                Ok(b) => b,
                Err(e) => {
                    emit(Event::Error {
                        text: format!(
                            "no track for battle {} round {} of {id}: {e}",
                            slot.index, slot.round
                        ),
                    });
                    continue;
                }
            };
            let headers: Vec<(&str, &str)> = slot
                .headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let mut tries = 0;
            loop {
                match self
                    .api
                    .put_presigned(&slot.url, headers.iter().copied(), &bytes)
                {
                    Ok(()) => {
                        uploaded.push(slot.slot.clone());
                        break;
                    }
                    Err(e) => {
                        tries += 1;
                        if tries >= UPLOAD_TRIES {
                            emit(Event::Error {
                                text: format!("uploading slot {}: {e}", slot.slot),
                            });
                            break;
                        }
                        if stop.aborting() || !sleep(Duration::from_secs(2 * tries as u64), stop) {
                            return PostEnd::Kept;
                        }
                    }
                }
            }
        }
        let mut confirmed = 0;
        for chunk in uploaded.chunks(RECORDINGS_DONE_MAX_SLOTS) {
            let request = RecordingsDoneRequest {
                batch: batch.request.batch.clone(),
                slots: chunk.to_vec(),
            };
            let mut tries = 0;
            loop {
                match self.api.recordings_done(&request) {
                    Ok(done) => {
                        confirmed += done.filled.len();
                        for failed in &done.failed {
                            emit(Event::Error {
                                text: format!("slot {}: {:?}", failed.slot, failed.error),
                            });
                        }
                        break;
                    }
                    Err(e) => {
                        if let Some(f) = fatal_of(&e) {
                            return PostEnd::Fatal(f);
                        }
                        tries += 1;
                        if tries >= UPLOAD_TRIES || !e.is_retryable() {
                            emit(Event::Error {
                                text: format!("confirming the recordings of {id}: {e}"),
                            });
                            break;
                        }
                        let wait = e
                            .retry_after()
                            .unwrap_or(Duration::from_secs(2 * tries as u64));
                        if stop.aborting() || !sleep(wait, stop) {
                            return PostEnd::Kept;
                        }
                    }
                }
            }
        }
        if !uploaded.is_empty() {
            emit(Event::Uploaded {
                batch: id.clone(),
                n: confirmed,
            });
        }
        self.pending.remove(id);
        PostEnd::Posted {
            accepted: response.accepted.len() as u32,
            rejected: response.rejected.len() as u32,
            uploaded: confirmed as u32,
        }
    }

    // ---- the loop ----

    fn run_loop(&self, stop: &Stop, emit: &mut dyn FnMut(Event), summary: &mut Summary) {
        let (tx, rx) = mpsc::channel::<Msg>();
        let slots = self.slots;
        std::thread::scope(|scope| {
            let mut batches: Vec<Batch> = Vec::new();
            let mut used = 0usize;
            let mut posting = 0usize;
            let mut fetching = false;
            let mut fetched = 0u32;
            let mut fetch_after = Instant::now();
            let mut backoff = Backoff::network();
            let mut next_key = 0u64;
            let mut stats = Throughput::default();
            let mut errors_in_a_row = 0u32;
            let mut fatal: Option<Fatal> = None;
            let mut stopping = false;
            let mut aborted = false;
            loop {
                // Stop requests and fatal errors.
                if !stopping && (stop.requested() || fatal.is_some()) {
                    stopping = true;
                    for b in &mut batches {
                        b.cut();
                    }
                }
                let hard = matches!(
                    fatal,
                    Some(
                        Fatal::LoginNeeded
                            | Fatal::Forbidden(_)
                            | Fatal::Engine(_)
                            | Fatal::ClientOutdated(_)
                    )
                );
                if (stop.aborting() || hard) && !aborted {
                    aborted = true;
                    for b in &batches {
                        b.abort.store(true, Ordering::Relaxed);
                    }
                }
                // Deadlines.
                let now = unix_now();
                for b in &mut batches {
                    if now + CUT_BEFORE_DEADLINE >= b.deadline {
                        b.cut();
                    }
                    if now + ABORT_BEFORE_DEADLINE >= b.deadline {
                        b.abort.store(true, Ordering::Relaxed);
                    }
                }

                // Batches whose battles are all settled go to the server.
                let mut i = 0;
                while i < batches.len() {
                    if !batches[i].complete() {
                        i += 1;
                        continue;
                    }
                    let batch = batches.remove(i);
                    if !batch.any_started {
                        emit(Event::Error {
                            text: format!("batch {} was never started, dropped", batch.id),
                        });
                        continue;
                    }
                    let (pending, tracks) =
                        batch.into_pending(self.config.options.env.sandbox.name());
                    posting += 1;
                    let tx = tx.clone();
                    scope.spawn(move || {
                        let events = tx.clone();
                        let end = self.post(&pending, &tracks, stop, &mut |e| {
                            let _ = events.send(Msg::Event(e));
                        });
                        let _ = tx.send(Msg::Posted(end));
                    });
                }

                // Ask for more work when the batches in hand are about to run dry.
                let unstarted: usize = batches.iter().map(Batch::unstarted).sum();
                if !stopping
                    && !fetching
                    && batches.len() < 2
                    && unstarted < slots
                    && Instant::now() >= fetch_after
                    && self.config.max_batches.is_none_or(|n| fetched < n)
                {
                    fetching = true;
                    let max = self.max_battles(stats.average());
                    let tx = tx.clone();
                    scope.spawn(move || {
                        let _ = tx.send(Msg::Fetched(self.fetch(max)));
                    });
                }

                // Start battles while slots are free.
                if !stopping {
                    'start: for b in &mut batches {
                        while let Some(pos) = b.next_unstarted() {
                            let cost = slot_cost(b.battles[pos].division).min(slots);
                            if used + cost > slots {
                                break 'start;
                            }
                            used += cost;
                            let battle = b.begin(pos);
                            emit(Event::BattleStarted {
                                batch: b.id.clone(),
                                index: battle.index,
                                division: battle.division,
                                bots: battle.bots.iter().map(robot_name).collect(),
                            });
                            let jars = b.jars[pos].clone();
                            let abort = b.abort.clone();
                            let (key, tx) = (b.key, tx.clone());
                            let runner = &self.runner;
                            scope.spawn(move || {
                                let progress = tx.clone();
                                let ran = catch_unwind(AssertUnwindSafe(|| {
                                    runner.run_with(&battle, &jars, Some(&abort), |p| {
                                        let Progress::Round { n, of } = p;
                                        let _ = progress.send(Msg::Progress { key, pos, n, of });
                                    })
                                }));
                                let outcome = ran.unwrap_or_else(|_| Outcome {
                                    result: status_only(battle.index, BattleStatus::Error),
                                    tracks: Vec::new(),
                                    why: Some("the battle thread panicked".into()),
                                    snapshots: Vec::new(),
                                });
                                let stopping = stop.requested() || abort.load(Ordering::Relaxed);
                                let outcome = unless_stopping(outcome, stopping);
                                let _ = tx.send(Msg::Battle {
                                    key,
                                    pos,
                                    outcome: Box::new(outcome),
                                });
                            });
                        }
                    }
                }

                // Done?
                let limit_reached = self.config.max_batches.is_some_and(|n| fetched >= n);
                if used == 0
                    && posting == 0
                    && !fetching
                    && batches.is_empty()
                    && (stopping || limit_reached)
                {
                    break;
                }

                // Messages.
                let mut next = rx.recv_timeout(TICK).ok();
                while let Some(msg) = next {
                    match msg {
                        Msg::Event(e) => emit(e),
                        Msg::Progress { key, pos, n, of } => {
                            if let Some(b) = batches.iter().find(|b| b.key == key) {
                                emit(Event::Round {
                                    batch: b.id.clone(),
                                    index: b.battles[pos].index,
                                    n,
                                    of,
                                });
                            }
                        }
                        Msg::Battle { key, pos, outcome } => {
                            let cost = batches
                                .iter()
                                .find(|b| b.key == key)
                                .map_or(1, |b| slot_cost(b.battles[pos].division).min(slots));
                            used = used.saturating_sub(cost);
                            if let Some(b) = batches.iter_mut().find(|b| b.key == key) {
                                let outcome = *outcome;
                                let status = outcome.result.status;
                                if let Some(ms) = outcome.result.duration_ms
                                    && status == BattleStatus::Ok
                                {
                                    stats.add(ms as f64 * cost as f64);
                                }
                                match status {
                                    BattleStatus::Ok => summary.battles_ok += 1,
                                    BattleStatus::RobotFailed => summary.robot_failed += 1,
                                    BattleStatus::NotRun => summary.not_run += 1,
                                    BattleStatus::Error => summary.errors += 1,
                                }
                                if status == BattleStatus::Error {
                                    errors_in_a_row += 1;
                                    let why = outcome.why.clone().unwrap_or_default();
                                    emit(Event::Error {
                                        text: format!(
                                            "battle {} failed here: {why}",
                                            b.battles[pos].index
                                        ),
                                    });
                                    if errors_in_a_row >= MAX_ERRORS_IN_A_ROW && fatal.is_none() {
                                        fatal = Some(Fatal::TooManyErrors(why));
                                    }
                                } else if status != BattleStatus::NotRun {
                                    errors_in_a_row = 0;
                                }
                                emit(b.done_event(pos, &outcome));
                                b.finish(pos, outcome);
                            }
                        }
                        Msg::Fetched(f) => {
                            fetching = false;
                            match f {
                                Fetched::Batch(p) if stopping => {
                                    // Not started, so not posted: it expires on the server.
                                    let _ = p;
                                }
                                Fetched::Batch(p) => {
                                    fetched += 1;
                                    if p.download_trouble {
                                        let wait = backoff.next(backoff::random_unit());
                                        fetch_after = Instant::now() + wait;
                                        emit(Event::Waiting {
                                            seconds: wait.as_secs(),
                                            why: "downloading robots failed".into(),
                                        });
                                    } else {
                                        backoff.reset();
                                    }
                                    next_key += 1;
                                    let (batch, premade) = Batch::new(next_key, *p);
                                    emit(Event::Batch {
                                        batch: batch.id.clone(),
                                        division: batch
                                            .battles
                                            .first()
                                            .map_or(Division::OneVsOne, |b| b.division),
                                        battles: batch.battles.len(),
                                        deadline: batch.deadline_text.clone(),
                                    });
                                    for (pos, note) in premade {
                                        let result = batch.results[pos].clone().unwrap();
                                        match result.status {
                                            BattleStatus::RobotFailed => summary.robot_failed += 1,
                                            BattleStatus::NotRun => summary.not_run += 1,
                                            _ => summary.errors += 1,
                                        }
                                        emit(Event::BattleDone {
                                            batch: batch.id.clone(),
                                            index: batch.battles[pos].index,
                                            division: batch.battles[pos].division,
                                            bots: batch.battles[pos]
                                                .bots
                                                .iter()
                                                .map(robot_name)
                                                .collect(),
                                            status: result.status,
                                            scores: Vec::new(),
                                            ranks: Vec::new(),
                                            duration_ms: None,
                                            detail: Some(note),
                                        });
                                    }
                                    batches.push(batch);
                                }
                                Fetched::Retry { after, why } => {
                                    let wait = match after {
                                        Some(a) => backoff::retry_after(a, backoff::random_unit()),
                                        None => backoff.next(backoff::random_unit()),
                                    };
                                    fetch_after = Instant::now() + wait;
                                    emit(Event::Waiting {
                                        seconds: wait.as_secs(),
                                        why,
                                    });
                                }
                                Fetched::Fatal(f) => {
                                    emit(Event::Error {
                                        text: f.to_string(),
                                    });
                                    fatal.get_or_insert(f);
                                }
                            }
                        }
                        Msg::Posted(end) => {
                            posting -= 1;
                            if let PostEnd::Fatal(f) = &end {
                                emit(Event::Error {
                                    text: f.to_string(),
                                });
                                fatal.get_or_insert(f.clone());
                            }
                            account(summary, end);
                        }
                    }
                    next = rx.try_recv().ok();
                }
            }
            if summary.fatal.is_none() {
                summary.fatal = fatal;
            }
        });
    }
}

/// Folds the end of a post into the summary; true when it was fatal.
fn account(summary: &mut Summary, end: PostEnd) -> bool {
    match end {
        PostEnd::Posted {
            accepted,
            rejected,
            uploaded,
        } => {
            summary.batches_posted += 1;
            summary.accepted += accepted;
            summary.rejected += rejected;
            summary.uploaded += uploaded;
            false
        }
        PostEnd::Dropped | PostEnd::Kept => false,
        PostEnd::Fatal(f) => {
            summary.fatal = Some(f);
            true
        }
    }
}

/// A battle's outcome, with a robot failure while the client was `stopping` (a stop request
/// or an abort) made `notRun`: the stop may be what ended the robot (a signal for the whole
/// process group or cgroup, #124), and a robot failure is reported only when the robot
/// failed.
fn unless_stopping(outcome: Outcome, stopping: bool) -> Outcome {
    let result = &outcome.result;
    if !stopping || result.status != BattleStatus::RobotFailed {
        return outcome;
    }
    let failed = result.failure.as_ref().map_or(String::new(), |f| {
        format!(": robot {} failed ({:?}) meanwhile", f.robot, f.reason)
    });
    Outcome {
        result: status_only(result.index, BattleStatus::NotRun),
        tracks: Vec::new(),
        why: Some(format!("stopped{failed}")),
        snapshots: Vec::new(),
    }
}

/// Sleeps `wait` unless an abort comes; false when it did.
fn sleep(wait: Duration, stop: &Stop) -> bool {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if stop.aborting() {
            return false;
        }
        std::thread::sleep(TICK.min(until.saturating_duration_since(Instant::now())));
    }
    !stop.aborting()
}

// ---- measuring ----

/// Slot time per battle, to size the next request.
#[derive(Default)]
struct Throughput {
    slot_ms: f64,
    battles: u32,
}

impl Throughput {
    fn add(&mut self, slot_ms: f64) {
        self.slot_ms += slot_ms;
        self.battles += 1;
    }

    fn average(&self) -> Option<f64> {
        (self.battles > 0).then(|| self.slot_ms / f64::from(self.battles))
    }
}

// ---- a batch in hand ----

struct Batch {
    key: u64,
    id: String,
    token: String,
    deadline: u64,
    deadline_text: String,
    battles: Vec<WorkBattle>,
    jars: Vec<Vec<PathBuf>>,
    /// Per battle position; `None` until it ended (or was cut).
    results: Vec<Option<BattleResult>>,
    tracks: Vec<Track>,
    started: Vec<bool>,
    any_started: bool,
    abort: Arc<AtomicBool>,
}

impl Batch {
    /// The batch, and the battles that never run (their position and why).
    fn new(key: u64, prepared: Prepared) -> (Batch, Vec<(usize, String)>) {
        let Prepared {
            work,
            battles,
            mut notes,
            ..
        } = prepared;
        let n = work.battles.len();
        let mut results = vec![None; n];
        let mut started = vec![false; n];
        let mut jars = vec![Vec::new(); n];
        let mut premade = Vec::new();
        let mut any_started = false;
        for (pos, battle) in battles.into_iter().enumerate() {
            match battle {
                Ok(paths) => jars[pos] = paths,
                Err(result) => {
                    started[pos] = true;
                    // Robots that failed to load are results; the rest are no reason to post.
                    any_started |= result.status != BattleStatus::NotRun;
                    results[pos] = Some(*result);
                    premade.push((pos, notes.remove(&pos).unwrap_or_default()));
                }
            }
        }
        let deadline_text = work.deadline.clone();
        let deadline = parse_iso(&deadline_text).unwrap_or_else(|| unix_now() + 3 * 3600);
        (
            Batch {
                key,
                id: work.batch_id,
                token: work.batch,
                deadline,
                deadline_text,
                battles: work.battles,
                jars,
                results,
                tracks: Vec::new(),
                started,
                any_started,
                abort: Arc::new(AtomicBool::new(false)),
            },
            premade,
        )
    }

    fn unstarted(&self) -> usize {
        self.started.iter().filter(|s| !**s).count()
    }

    fn next_unstarted(&self) -> Option<usize> {
        self.started.iter().position(|s| !*s)
    }

    fn begin(&mut self, pos: usize) -> WorkBattle {
        self.started[pos] = true;
        self.any_started = true;
        self.battles[pos].clone()
    }

    /// The battles not started become `notRun` (stop, or the deadline near).
    fn cut(&mut self) {
        for pos in 0..self.battles.len() {
            if !self.started[pos] {
                self.started[pos] = true;
                self.results[pos] =
                    Some(status_only(self.battles[pos].index, BattleStatus::NotRun));
            }
        }
    }

    fn finish(&mut self, pos: usize, outcome: Outcome) {
        let index = self.battles[pos].index;
        for (round, bytes) in outcome.tracks.into_iter().enumerate() {
            self.tracks.push(Track {
                index,
                round: round as u32,
                bytes,
            });
        }
        self.results[pos] = Some(outcome.result);
    }

    fn complete(&self) -> bool {
        self.results.iter().all(Option::is_some)
    }

    fn done_event(&self, pos: usize, outcome: &Outcome) -> Event {
        let battle = &self.battles[pos];
        let robots = outcome.result.robots.as_deref().unwrap_or(&[]);
        Event::BattleDone {
            batch: self.id.clone(),
            index: battle.index,
            division: battle.division,
            bots: battle.bots.iter().map(robot_name).collect(),
            status: outcome.result.status,
            scores: robots.iter().map(|r| r.score).collect(),
            ranks: robots.iter().map(|r| r.rank).collect(),
            duration_ms: outcome.result.duration_ms,
            detail: outcome
                .result
                .failure
                .as_ref()
                .map(|f| match &f.detail {
                    Some(d) => format!(
                        "{} {:?}: {d}",
                        battle.bots[f.robot as usize % battle.bots.len()].id,
                        f.reason
                    ),
                    None => format!("{:?}", f.reason),
                })
                .or_else(|| outcome.why.clone()),
        }
    }

    /// The request, and the tracks that wait for the upload slots.
    fn into_pending(self, sandbox: &str) -> (PendingBatch, Vec<Track>) {
        let release = env!("CARGO_PKG_VERSION");
        let request = ResultsRequest {
            batch: self.token,
            engine_version: ENGINE_VERSION,
            client: ClientInfo {
                release: release.to_string(),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                sandbox: sandbox.to_string(),
            },
            battles: self.results.into_iter().flatten().collect(),
        };
        (
            PendingBatch {
                batch_id: self.id,
                deadline: self.deadline_text,
                request,
            },
            self.tracks,
        )
    }
}

fn status_only(index: u32, status: BattleStatus) -> BattleResult {
    BattleResult {
        index,
        status,
        duration_ms: None,
        battle_hash: None,
        robots: None,
        rounds: None,
        failure: None,
    }
}

fn robot_failed(index: u32, robot: usize, detail: &str) -> BattleResult {
    let mut result = status_only(index, BattleStatus::RobotFailed);
    let detail: String = detail.chars().take(500).collect();
    result.failure = Some(Failure {
        robot: robot as u32,
        reason: FailureReason::Load,
        detail: Some(detail),
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenfile::iso;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rumble-contrib-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn batches_are_sized_for_twenty_minutes() {
        // No measurement yet: the least.
        assert_eq!(batch_size(None, 4, 50), MIN_BATCH);
        // 30 s per battle on one slot, 4 slots: 20 min * 4 / 30 s = 160, capped.
        assert_eq!(batch_size(Some(30_000.0), 4, 50), 50);
        // 5 minutes per battle: 4 slots take 16 in 20 minutes.
        assert_eq!(batch_size(Some(300_000.0), 4, 50), 16);
        // Very slow battles: never below the least.
        assert_eq!(batch_size(Some(3_600_000.0), 1, 50), MIN_BATCH);
        // The cap of melee.
        assert_eq!(batch_size(Some(1_000.0), 8, MAX_BATCH_MELEE), 10);
        assert_eq!(batch_size(Some(f64::NAN), 8, 50), MIN_BATCH);
    }

    #[test]
    fn melee_is_offered_with_five_slots() {
        let both = Division::ALL.to_vec();
        assert_eq!(offered_divisions(&both, 4), [Division::OneVsOne]);
        assert_eq!(offered_divisions(&both, 5), both);
        assert_eq!(offered_divisions(&[Division::Melee], 2), []);
        assert_eq!(offered_divisions(&[Division::Melee], 6), [Division::Melee]);
    }

    #[test]
    fn releases_compare_by_number() {
        assert!(release_at_least("0.2.0", "0.2.0"));
        assert!(release_at_least("0.10.0", "0.9.9"));
        assert!(release_at_least("1.0", "0.9.9"));
        assert!(!release_at_least("0.1.0", "0.2.0"));
        assert!(release_at_least("0.2.0-rc1", "0.2.0"));
        assert!(!release_at_least("0.1.9", "0.2"));
    }

    #[test]
    fn iso_times_round_trip() {
        for secs in [0, 86_399, 1_792_854_000, 1_709_164_800, 4_102_444_800] {
            assert_eq!(parse_iso(&iso(secs)), Some(secs), "{secs}");
        }
        assert_eq!(parse_iso("2026-10-24T15:00:00Z"), Some(1_792_854_000));
        assert_eq!(parse_iso("2026-10-24T15:00:00.250Z"), Some(1_792_854_000));
        assert_eq!(parse_iso("2026-10-24 15:00:00"), None);
        assert_eq!(parse_iso("2026-13-24T15:00:00Z"), None);
    }

    #[test]
    fn stop_has_two_stages() {
        let stop = Stop::new();
        assert!(!stop.requested() && !stop.aborting());
        let copy = stop.clone();
        copy.stop();
        assert!(stop.requested() && !stop.aborting());
        stop.stop();
        stop.stop();
        assert!(copy.aborting());
    }

    #[test]
    fn a_robot_crash_while_stopping_is_not_reported() {
        let outcome = |status| {
            let mut result = robot_failed(3, 1, "exit 130");
            result.failure.as_mut().unwrap().reason = FailureReason::Crash;
            if status != BattleStatus::RobotFailed {
                result = status_only(3, status);
            }
            Outcome {
                result,
                tracks: vec![vec![1]],
                why: None,
                snapshots: Vec::new(),
            }
        };
        let stopped = unless_stopping(outcome(BattleStatus::RobotFailed), true);
        assert_eq!(stopped.result, status_only(3, BattleStatus::NotRun));
        assert!(stopped.tracks.is_empty());
        assert_eq!(
            stopped.why.as_deref(),
            Some("stopped: robot 1 failed (Crash) meanwhile")
        );
        let crashed = unless_stopping(outcome(BattleStatus::RobotFailed), false);
        assert_eq!(crashed.result.status, BattleStatus::RobotFailed);
        // Finished battles are posted as they are.
        let ok = unless_stopping(outcome(BattleStatus::Ok), true);
        assert_eq!(ok.result.status, BattleStatus::Ok);
        assert_eq!(ok.tracks, [vec![1]]);
    }

    #[test]
    fn slots_default_to_at_least_one() {
        assert!(default_slots() >= 1);
    }

    #[test]
    fn events_serialize_for_json_lines() {
        let e = Event::BattleDone {
            batch: "b-1".into(),
            index: 2,
            division: Division::OneVsOne,
            bots: vec!["a.B 1".into(), "c.D 2".into()],
            status: BattleStatus::Ok,
            scores: vec![60.0, 40.0],
            ranks: vec![1, 2],
            duration_ms: Some(1500),
            detail: None,
        };
        let json: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(json["event"], "battleDone");
        assert_eq!(json["durationMs"], 1500);
        assert_eq!(json["division"], "1v1");
        assert_eq!(json["status"], "ok");
        let w = serde_json::to_value(Event::Waiting {
            seconds: 30,
            why: "no work".into(),
        })
        .unwrap();
        assert_eq!(
            w,
            serde_json::json!({"event": "waiting", "seconds": 30, "why": "no work"})
        );
    }

    fn pending(id: &str) -> PendingBatch {
        PendingBatch {
            batch_id: id.into(),
            deadline: "2026-10-24T15:00:00Z".into(),
            request: ResultsRequest {
                batch: "tok.en".into(),
                engine_version: 1,
                client: ClientInfo {
                    release: "0.2.0".into(),
                    os: "linux".into(),
                    arch: "x86_64".into(),
                    sandbox: "os".into(),
                },
                battles: vec![status_only(0, BattleStatus::NotRun)],
            },
        }
    }

    #[test]
    fn pending_batches_are_written_listed_and_removed() {
        let dir = temp("pending");
        let pending_dir = PendingDir::in_data_dir(&dir);
        assert!(pending_dir.list().is_empty());
        let tracks = vec![
            Track {
                index: 0,
                round: 0,
                bytes: vec![1, 2, 3],
            },
            Track {
                index: 1,
                round: 4,
                bytes: vec![9],
            },
        ];
        pending_dir.write(&pending("b-01/x"), &tracks).unwrap();
        pending_dir.write(&pending("b-02"), &[]).unwrap();
        let listed = pending_dir.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0], pending("b-01/x"));
        assert_eq!(pending_dir.read_track("b-01/x", 1, 4).unwrap(), [9]);
        // Junk: an unreadable request and tracks without a request go.
        fs::write(pending_dir.path().join("junk.json"), "{").unwrap();
        fs::create_dir_all(pending_dir.path().join("b-99")).unwrap();
        assert_eq!(pending_dir.list().len(), 2);
        assert!(!pending_dir.path().join("junk.json").exists());
        assert!(!pending_dir.path().join("b-99").exists());
        pending_dir.remove("b-01/x");
        assert_eq!(pending_dir.list().len(), 1);
        assert!(pending_dir.read_track("b-01/x", 0, 0).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_stays_under_a_mebibyte() {
        let dir = temp("log");
        let mut log = Log::open(&dir);
        let line = "x".repeat(1000);
        for i in 0..3000 {
            log.line(&format!("{i} {line}"));
        }
        let size = fs::metadata(dir.join("log.txt")).unwrap().len();
        assert!(size <= LOG_CAP, "{size}");
        assert!(size > LOG_CAP / 4);
        let text = fs::read_to_string(dir.join("log.txt")).unwrap();
        assert!(text.lines().last().unwrap().contains("2999 "));
        // Whole lines only.
        assert!(text.lines().all(|l| l.contains('Z')));
        // A new Log continues from the size on disk.
        let mut again = Log::open(&dir);
        again.line("more");
        assert!(fs::metadata(dir.join("log.txt")).unwrap().len() <= LOG_CAP);
        let _ = fs::remove_dir_all(dir);
    }
}
