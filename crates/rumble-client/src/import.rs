// SPDX-License-Identifier: EPL-2.0

//! The seeding importer (docs/rumble-backend.md §16): reads a RoboWiki participants page,
//! finds a valid jar for every entry (local folder, the list's URL, the archive mirror),
//! validates it with the server's own code and registers it through the admin endpoints.
//!
//! Library part only; `robocode-s rumble admin import` wraps [`run`].

use crate::backoff;
use crate::http::{Api, Error as ApiError};
use rumble_core::Config;
use rumble_core::api::{UploadUrlRequest, UploadUrlResponse};
use rumble_core::ids::Division;
use rumble_core::jar::{JarInfo, validate_named};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// RoboWiki's 1v1 participants page, raw.
pub const LIST_1V1: &str =
    "https://robowiki.net/w/index.php?title=RoboRumble/Participants&action=raw";
/// RoboWiki's melee participants page, raw.
pub const LIST_MELEE: &str =
    "https://robowiki.net/w/index.php?title=RoboRumble/Participants/Melee&action=raw";
/// The archive the classic rumble clients fall back on.
pub const DEFAULT_MIRROR: &str =
    "https://robocode-archive.strangeautomata.com/robots/{id}_{version}.jar";

/// Download limit per jar (§16.1).
pub const MAX_JAR_BYTES: u64 = 5 << 20;
/// The page itself.
const MAX_LIST_BYTES: u64 = 16 << 20;

/// The list URL of a division.
pub fn default_list(division: Division) -> &'static str {
    match division {
        Division::OneVsOne => LIST_1V1,
        Division::Melee => LIST_MELEE,
    }
}

// ---- the list ----

/// One line of the list: `<id> <version>,<url>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: String,
    pub version: String,
    pub url: String,
    /// 1-based line in the page.
    pub line: usize,
}

/// A line that is not an entry, or a repeated one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub line: usize,
    pub text: String,
}

/// What a participants page holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ParsedList {
    pub entries: Vec<Entry>,
    /// Lines repeating an earlier `id version`; the first one counts.
    pub duplicates: Vec<Problem>,
    pub malformed: Vec<Problem>,
}

/// Parses a page in `action=raw` form. Entries are the lines between `<pre>` and `</pre>`
/// (all lines if there is no `<pre>`, for plain files). Each is `<id> <version>,<url>`: the id
/// ends at the first space, the version at the first comma after it (versions may hold
/// spaces, never commas; the URL may hold anything). Blank and `#` lines are skipped.
pub fn parse_list(text: &str) -> ParsedList {
    let mut out = ParsedList::default();
    let mut seen = HashSet::new();
    let mut inside = !text.contains("<pre>");
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let mut raw = raw;
        if !inside {
            match raw.find("<pre>") {
                Some(at) => {
                    inside = true;
                    raw = &raw[at + 5..];
                }
                None => continue,
            }
        }
        let mut done = false;
        if let Some(at) = raw.find("</pre>") {
            raw = &raw[..at];
            done = true;
        }
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            match parse_entry(trimmed, line) {
                Some(entry) => {
                    if seen.insert((entry.id.clone(), entry.version.clone())) {
                        out.entries.push(entry);
                    } else {
                        out.duplicates.push(Problem {
                            line,
                            text: trimmed.to_string(),
                        });
                    }
                }
                None => out.malformed.push(Problem {
                    line,
                    text: trimmed.to_string(),
                }),
            }
        }
        if done {
            break;
        }
    }
    out
}

fn parse_entry(text: &str, line: usize) -> Option<Entry> {
    let (id, rest) = text.split_once(' ')?;
    let (version, url) = rest.split_once(',')?;
    let (version, url) = (version.trim(), url.trim());
    if id.is_empty() || version.is_empty() || url.is_empty() || id.contains(',') {
        return None;
    }
    Some(Entry {
        id: id.to_string(),
        version: version.to_string(),
        url: url.to_string(),
        line,
    })
}

// ---- fetching ----

/// Gets the bytes of a URL (the whole policy: timeouts, retries, limit, redirects) or says
/// why not, in the words of the report (`<status or reason>`).
pub trait Fetcher: Sync {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String>;
}

/// The real thing: ureq, 30 s timeout, redirects followed, http allowed, 2 retries.
pub struct HttpFetcher {
    agent: ureq::Agent,
    retries: u32,
}

impl Default for HttpFetcher {
    fn default() -> Self {
        HttpFetcher::new()
    }
}

impl HttpFetcher {
    pub fn new() -> HttpFetcher {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(8)
            .build()
            .into();
        HttpFetcher { agent, retries: 2 }
    }

    fn once(&self, url: &str, limit: u64) -> Result<Vec<u8>, (String, bool)> {
        let mut response = self
            .agent
            .get(url)
            .header("User-Agent", "robocode-s-importer")
            .call()
            .map_err(|e| match e {
                ureq::Error::StatusCode(code) => {
                    (format!("HTTP {code}"), code >= 500 || code == 429)
                }
                other => (other.to_string(), true),
            })?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| {
                let text = e.to_string();
                if text.contains("limit") {
                    (format!("larger than {limit} bytes"), false)
                } else {
                    (text, true)
                }
            })
    }
}

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let mut attempt = 0;
        loop {
            match self.once(url, limit) {
                Ok(bytes) => return Ok(bytes),
                Err((why, retry)) => {
                    if !retry || attempt >= self.retries {
                        return Err(why);
                    }
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
                }
            }
        }
    }
}

/// The list page: a file, or a URL fetched with `fetcher`.
pub fn read_list(source: &str, fetcher: &dyn Fetcher) -> Result<String, String> {
    let bytes = if source.starts_with("http://") || source.starts_with("https://") {
        fetcher.fetch(source, MAX_LIST_BYTES)?
    } else {
        std::fs::read(source).map_err(|e| format!("{source}: {e}"))?
    };
    String::from_utf8(bytes).or_else(|e| Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

// ---- resolving one entry ----

/// Where a jar came from; the `source` of `admin/bots/import`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Local,
    List,
    Mirror,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Local => "local",
            Source::List => "list",
            Source::Mirror => "mirror",
        }
    }
}

/// Settings of a run.
#[derive(Clone, Debug)]
pub struct Options {
    pub division: Division,
    /// Folder of `<id>_<version>.jar` files.
    pub local: Option<PathBuf>,
    /// URL template with `{id}` and `{version}`; `None` turns the mirror off.
    pub mirror: Option<String>,
    /// Downloads and imports at once.
    pub concurrency: usize,
    /// Download and validate only; no API calls.
    pub dry_run: bool,
    pub config: Config,
}

impl Options {
    pub fn new(division: Division) -> Options {
        Options {
            division,
            local: None,
            mirror: Some(DEFAULT_MIRROR.to_string()),
            concurrency: 4,
            dry_run: false,
            config: Config::default(),
        }
    }
}

/// A jar that passed.
#[derive(Clone, Debug)]
pub struct Found {
    pub bytes: Vec<u8>,
    pub info: JarInfo,
    pub source: Source,
    /// The properties had no `robot.version`; the list's was used.
    pub version_from_list: bool,
    /// Where it was fetched (the entry's own URL for local files).
    pub source_url: String,
}

/// The `{id}_{version}.jar` URL of the mirror template (version percent-encoded).
pub fn mirror_url(template: &str, id: &str, version: &str) -> String {
    template
        .replace("{id}", id)
        .replace("{version}", &percent_encode(version))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The jar must be valid and say it is this entry. A missing `robot.version` takes the
/// list's (classic's list is authoritative); a different one is a mismatch.
fn check(bytes: &[u8], entry: &Entry, config: &Config) -> Result<(JarInfo, bool), String> {
    let (info, from_list) = validate_named(bytes, config, Some(&entry.version), &entry.id)
        .map_err(|messages| format!("invalid ({})", messages.join("; ")))?;
    if info.bot_id != entry.id || info.version != entry.version {
        return Err(format!(
            "mismatch (properties say {} {})",
            info.bot_id, info.version
        ));
    }
    Ok((info, from_list))
}

/// The first source that yields a valid jar for `entry`: local, list URL, mirror. When none
/// does, the error is the most telling one: the first `invalid`/`mismatch`, else the last
/// download failure.
pub fn resolve(entry: &Entry, options: &Options, fetcher: &dyn Fetcher) -> Result<Found, String> {
    let mut errors: Vec<String> = Vec::new();
    let mut found = |bytes: Vec<u8>, source: Source, url: &str| -> Option<Found> {
        match check(&bytes, entry, &options.config) {
            Ok((info, version_from_list)) => Some(Found {
                bytes,
                info,
                source,
                version_from_list,
                source_url: url.to_string(),
            }),
            Err(why) => {
                errors.push(why);
                None
            }
        }
    };

    if let Some(dir) = &options.local {
        let path = dir.join(format!("{}_{}.jar", entry.id, entry.version));
        if let Ok(bytes) = std::fs::read(&path)
            && let Some(f) = found(bytes, Source::Local, &entry.url)
        {
            return Ok(f);
        }
    }
    let mut tried = Vec::new();
    let mirror = options
        .mirror
        .as_deref()
        .map(|t| mirror_url(t, &entry.id, &entry.version));
    let candidates = [
        (Some(entry.url.clone()), Source::List),
        (mirror, Source::Mirror),
    ];
    let mut download_error = None;
    for (url, source) in candidates {
        let Some(url) = url else { continue };
        if tried.contains(&url) {
            continue;
        }
        tried.push(url.clone());
        match fetcher.fetch(&url, MAX_JAR_BYTES) {
            Ok(bytes) => {
                if let Some(f) = found(bytes, source, &url) {
                    return Ok(f);
                }
            }
            Err(why) => download_error = Some(format!("download ({why})")),
        }
    }
    Err(errors
        .into_iter()
        .next()
        .or(download_error)
        .unwrap_or_else(|| "download (no source)".to_string()))
}

// ---- the report ----

/// One row of the report (§16.3).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryReport {
    pub id: String,
    pub version: String,
    /// `created`, `exists`, `divisionAdded`, `valid` (dry run) or `failed`.
    pub result: String,
    pub source: Option<String>,
    pub sha256: Option<String>,
    pub codesize: Option<u32>,
    pub weight_class: Option<String>,
    /// `download (..)`, `invalid (..)`, `mismatch (..)`, `conflict`, `upload (..)`, `api (..)`.
    pub error: Option<String>,
    /// The jar's properties had no version; the list's was used.
    pub version_from_list: bool,
}

/// The whole report: written as JSON, summarized for the terminal.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub division: String,
    pub dry_run: bool,
    pub counts: BTreeMap<String, usize>,
    pub entries: Vec<EntryReport>,
    pub duplicates: Vec<Problem>,
    pub malformed: Vec<Problem>,
}

impl Report {
    fn new(
        division: Division,
        dry_run: bool,
        entries: Vec<EntryReport>,
        list: &ParsedList,
    ) -> Report {
        let mut counts = BTreeMap::new();
        for e in &entries {
            *counts.entry(e.result.clone()).or_insert(0) += 1;
        }
        Report {
            division: division.as_str().to_string(),
            dry_run,
            counts,
            entries,
            duplicates: list.duplicates.clone(),
            malformed: list.malformed.clone(),
        }
    }

    pub fn failures(&self) -> impl Iterator<Item = &EntryReport> {
        self.entries.iter().filter(|e| e.result == "failed")
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    /// Counts per result, how many came from where, and the failures.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} entries ({}{})\n",
            self.entries.len(),
            self.division,
            if self.dry_run { ", dry run" } else { "" }
        );
        for (result, n) in &self.counts {
            out.push_str(&format!("  {result}: {n}\n"));
        }
        let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
        for s in self.entries.iter().filter_map(|e| e.source.as_deref()) {
            *sources.entry(s).or_insert(0) += 1;
        }
        if !sources.is_empty() {
            let parts: Vec<_> = sources.iter().map(|(s, n)| format!("{s} {n}")).collect();
            out.push_str(&format!("  sources: {}\n", parts.join(", ")));
        }
        let from_list = self.entries.iter().filter(|e| e.version_from_list).count();
        if from_list > 0 {
            out.push_str(&format!("  version taken from the list: {from_list}\n"));
        }
        if !self.duplicates.is_empty() {
            out.push_str(&format!("  duplicate lines: {}\n", self.duplicates.len()));
        }
        if !self.malformed.is_empty() {
            out.push_str(&format!("  malformed lines: {}\n", self.malformed.len()));
        }
        let failures: Vec<_> = self.failures().collect();
        if !failures.is_empty() {
            out.push_str("failures:\n");
            for e in failures {
                out.push_str(&format!(
                    "  {} {}: {}\n",
                    e.id,
                    e.version,
                    e.error.as_deref().unwrap_or("?")
                ));
            }
        }
        out
    }
}

/// `import-<division>-<date>.json`, the default report name (`date` as `YYYY-MM-DD`).
pub fn default_report_name(division: Division, date: &str) -> String {
    format!("import-{}-{date}.json", division.as_str())
}

/// Today (UTC) as `YYYY-MM-DD`.
pub fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

// ---- the run ----

/// Called as each entry finishes: the row, how many are done, how many there are.
pub type Progress<'a> = &'a (dyn Fn(&EntryReport, usize, usize) + Sync);

fn failed(entry: &Entry, error: String) -> EntryReport {
    EntryReport {
        id: entry.id.clone(),
        version: entry.version.clone(),
        result: "failed".to_string(),
        source: None,
        sha256: None,
        codesize: None,
        weight_class: None,
        error: Some(error),
        version_from_list: false,
    }
}

/// Attempts per API call before the entry fails.
const ATTEMPTS: u32 = 6;

/// The importer's own pace: API calls start at least `gap` apart, over all worker threads.
pub struct Pacer {
    gap: Duration,
    next: Mutex<Instant>,
}

impl Pacer {
    pub fn new(gap: Duration) -> Pacer {
        Pacer {
            gap,
            next: Mutex::new(Instant::now()),
        }
    }

    /// Two calls a second: the server's limit for other calls is 2/s (spec section 5.3).
    pub fn api() -> Pacer {
        Pacer::new(Duration::from_millis(500))
    }

    /// Blocks until this caller's slot.
    pub fn wait(&self) {
        let slot = {
            let mut next = self.next.lock().unwrap();
            let slot = (*next).max(Instant::now());
            *next = slot + self.gap;
            slot
        };
        let now = Instant::now();
        if slot > now {
            std::thread::sleep(slot - now);
        }
    }
}

/// Calls `call` paced, again on retryable errors after the server's `Retry-After` (with
/// jitter), up to [`ATTEMPTS`] attempts.
fn retrying<T>(
    pacer: &Pacer,
    mut call: impl FnMut() -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let mut attempt = 1;
    loop {
        pacer.wait();
        match call() {
            Err(e) if e.is_retryable() && attempt < ATTEMPTS => {
                let base = e
                    .retry_after()
                    .unwrap_or(Duration::from_secs(1 << attempt.min(5)))
                    .min(Duration::from_secs(60));
                std::thread::sleep(backoff::retry_after(base, backoff::random_unit()));
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Uploads the jar unless the server has it, then registers it. Returns the server's result.
fn register(
    api: &Api,
    pacer: &Pacer,
    found: &Found,
    entry: &Entry,
    division: Division,
) -> Result<String, String> {
    let request = UploadUrlRequest {
        sha256: found.info.sha256.clone(),
        size: found.info.size,
    };
    // `{"exists": true}` has no URL, so it is read as plain JSON.
    let answer: Value = retrying(pacer, || api.post_json("admin/jars/upload-url", &request))
        .map_err(|e| format!("upload ({e})"))?;
    if answer.get("exists").and_then(Value::as_bool) != Some(true) {
        let upload: UploadUrlResponse = serde_json::from_value(answer)
            .map_err(|e| format!("upload (unreadable upload-url answer: {e})"))?;
        let headers = upload.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()));
        let headers: Vec<_> = headers.collect();
        retrying(pacer, || {
            api.put_presigned(&upload.url, headers.iter().copied(), &found.bytes)
        })
        .map_err(|e| format!("upload ({e})"))?;
    }
    let mut body = json!({
        "bot": found.info.bot_id,
        "version": found.info.version,
        "sha256": found.info.sha256,
        "division": division.as_str(),
        "author": found.info.author,
        "sourceUrl": found.source_url,
        "source": found.source.as_str(),
    });
    if found.version_from_list {
        body["versionFromList"] = json!(true);
    }
    debug_assert_eq!(found.info.bot_id, entry.id);
    let answer = retrying(pacer, || api.admin_import(&body)).map_err(|e| match e {
        ApiError::Conflict(_) => "conflict".to_string(),
        other => format!("api ({other})"),
    })?;
    match answer.get("result").and_then(Value::as_str) {
        Some(r @ ("created" | "exists" | "divisionAdded")) => Ok(r.to_string()),
        other => Err(format!("api (unexpected result {other:?})")),
    }
}

fn process(
    entry: &Entry,
    options: &Options,
    fetcher: &dyn Fetcher,
    api: Option<&Api>,
    pacer: &Pacer,
) -> EntryReport {
    let found = match resolve(entry, options, fetcher) {
        Ok(f) => f,
        Err(why) => return failed(entry, why),
    };
    let mut row = EntryReport {
        id: entry.id.clone(),
        version: found.info.version.clone(),
        result: "valid".to_string(),
        source: Some(found.source.as_str().to_string()),
        sha256: Some(found.info.sha256.clone()),
        codesize: Some(found.info.codesize),
        weight_class: Some(found.info.weight_class.as_str().to_string()),
        error: None,
        version_from_list: found.version_from_list,
    };
    if let (false, Some(api)) = (options.dry_run, api) {
        match register(api, pacer, &found, entry, options.division) {
            Ok(result) => row.result = result,
            Err(why) => {
                row.result = "failed".to_string();
                row.error = Some(why);
            }
        }
    }
    row
}

/// Imports every entry: `api` is required unless `options.dry_run`. Entries are worked off by
/// `options.concurrency` threads; the report lists them in list order.
pub fn run(
    list: &ParsedList,
    options: &Options,
    fetcher: &dyn Fetcher,
    api: Option<&Api>,
    progress: Progress,
) -> Report {
    let total = list.entries.len();
    let pacer = Pacer::api();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let rows: Mutex<Vec<Option<EntryReport>>> = Mutex::new(vec![None; total]);
    let workers = options.concurrency.clamp(1, 64).min(total.max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(entry) = list.entries.get(i) else {
                        break;
                    };
                    let row = if api.is_none() && !options.dry_run {
                        failed(entry, "api (no API client)".to_string())
                    } else {
                        process(entry, options, fetcher, api, &pacer)
                    };
                    let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                    progress(&row, n, total);
                    rows.lock().unwrap()[i] = Some(row);
                }
            });
        }
    });
    let entries = rows.into_inner().unwrap().into_iter().flatten().collect();
    Report::new(options.division, options.dry_run, entries, list)
}

/// Reads the file `path` or fetches the list, then parses it.
pub fn load_list(source: &str, fetcher: &dyn Fetcher) -> Result<ParsedList, String> {
    read_list(source, fetcher).map(|t| parse_list(&t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const ONE_V_ONE: &str = include_str!("../tests/fixtures/participants-1v1.txt");
    const MELEE: &str = include_str!("../tests/fixtures/participants-melee.txt");
    const SAMPLE_JAR: &[u8] =
        include_bytes!("../../rumble-core/tests/fixtures/samples/sample.MyFirstRobot.jar");

    #[test]
    fn the_1v1_page_parses() {
        let list = parse_list(ONE_V_ONE);
        assert!(list.malformed.is_empty(), "{:?}", list.malformed);
        assert!(list.duplicates.is_empty(), "{:?}", list.duplicates);
        assert!(list.entries.len() > 1000, "{}", list.entries.len());
        let first = &list.entries[0];
        assert_eq!(
            (first.id.as_str(), first.version.as_str()),
            ("a.FreyaOS", "1.0")
        );
        assert_eq!(
            first.url,
            "http://robocode-archive.strangeautomata.com/robots/a.FreyaOS_1.0.jar"
        );
        // Query strings with commas-free `&` and `=` stay whole.
        let scalar = list
            .entries
            .iter()
            .find(|e| e.id == "aaa.r.ScalarR")
            .unwrap();
        assert_eq!(scalar.version, "0.005h.053-noshield");
        assert!(scalar.url.contains("export=download&id="));
        // The notes after </pre> are not entries.
        assert!(!list.entries.iter().any(|e| e.id.starts_with("'''")));
    }

    #[test]
    fn the_melee_page_parses() {
        let list = parse_list(MELEE);
        assert!(list.malformed.is_empty(), "{:?}", list.malformed);
        assert!(list.entries.len() > 300, "{}", list.entries.len());
        // Both lists name some bots twice over (one bot, two divisions).
        let one = parse_list(ONE_V_ONE);
        let both = list
            .entries
            .iter()
            .filter(|m| {
                one.entries
                    .iter()
                    .any(|o| o.id == m.id && o.version == m.version)
            })
            .count();
        assert!(both > 100, "{both}");
    }

    #[test]
    fn duplicates_malformed_and_comments() {
        let text = "intro\n<pre>\n# a comment\n\na.B 1.0,http://x/a.jar\nc.D 2 beta,http://x/c,d.jar\na.B 1.0,http://y/a.jar\nnospace,http://x\nno.Comma 1.0 http://x\nbad. ,http://x\ne.F 1,\na.B 1.1,http://x/a11.jar\n</pre>\nz.Z 1,http://after\n";
        let list = parse_list(text);
        let ids: Vec<_> = list
            .entries
            .iter()
            .map(|e| format!("{} {} {}", e.id, e.version, e.url))
            .collect();
        assert_eq!(
            ids,
            [
                "a.B 1.0 http://x/a.jar",
                "c.D 2 beta http://x/c,d.jar",
                "a.B 1.1 http://x/a11.jar"
            ]
        );
        assert_eq!(
            list.duplicates,
            [Problem {
                line: 7,
                text: "a.B 1.0,http://y/a.jar".into()
            }]
        );
        let bad: Vec<_> = list.malformed.iter().map(|p| p.line).collect();
        assert_eq!(bad, [8, 9, 10, 11]);
        assert_eq!(list.entries[0].line, 5);
    }

    #[test]
    fn plain_files_without_pre() {
        let list = parse_list("a.B 1.0,http://x\n# c\n");
        assert_eq!(list.entries.len(), 1);
    }

    #[test]
    fn mirror_template() {
        assert_eq!(
            mirror_url(DEFAULT_MIRROR, "a.B", "1.0 beta+2"),
            "https://robocode-archive.strangeautomata.com/robots/a.B_1.0%20beta%2B2.jar"
        );
    }

    struct Fake {
        files: HashMap<String, Result<Vec<u8>, String>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(files: &[(&str, Result<&[u8], &str>)]) -> Fake {
            Fake {
                files: files
                    .iter()
                    .map(|(u, r)| (u.to_string(), r.map(<[u8]>::to_vec).map_err(str::to_string)))
                    .collect(),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetcher for Fake {
        fn fetch(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
            self.calls.lock().unwrap().push(url.to_string());
            self.files
                .get(url)
                .cloned()
                .unwrap_or_else(|| Err("HTTP 404".to_string()))
        }
    }

    fn entry() -> Entry {
        Entry {
            id: "sample.MyFirstRobot".into(),
            version: "1.0".into(),
            url: "http://list/x.jar".into(),
            line: 1,
        }
    }

    fn options(local: Option<PathBuf>) -> Options {
        let mut o = Options::new(Division::OneVsOne);
        o.local = local;
        o.mirror = Some("http://mirror/{id}_{version}.jar".into());
        o
    }

    #[test]
    fn sources_in_order_and_version_fallback() {
        let dir = std::env::temp_dir().join(format!("rumble-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.MyFirstRobot_1.0.jar");

        // Local first; nothing is downloaded.
        std::fs::write(&file, SAMPLE_JAR).unwrap();
        let fake = Fake::new(&[]);
        let found = resolve(&entry(), &options(Some(dir.clone())), &fake).unwrap();
        assert_eq!(found.source, Source::Local);
        assert_eq!(found.source_url, "http://list/x.jar");
        assert!(found.version_from_list, "the sample has no robot.version");
        assert_eq!(found.info.version, "1.0");
        assert!(fake.calls().is_empty());

        // A broken local file falls through to the list URL.
        std::fs::write(&file, b"not a jar").unwrap();
        let fake = Fake::new(&[("http://list/x.jar", Ok(SAMPLE_JAR))]);
        let found = resolve(&entry(), &options(Some(dir.clone())), &fake).unwrap();
        assert_eq!(found.source, Source::List);
        assert_eq!(fake.calls(), ["http://list/x.jar"]);

        // List fails, the mirror is next.
        let fake = Fake::new(&[
            ("http://list/x.jar", Err("HTTP 503")),
            ("http://mirror/sample.MyFirstRobot_1.0.jar", Ok(SAMPLE_JAR)),
        ]);
        let found = resolve(&entry(), &options(None), &fake).unwrap();
        assert_eq!(found.source, Source::Mirror);
        assert_eq!(
            fake.calls(),
            [
                "http://list/x.jar",
                "http://mirror/sample.MyFirstRobot_1.0.jar"
            ]
        );

        // An entry whose list URL is the mirror URL is fetched once.
        let mut same = entry();
        same.url = "http://mirror/sample.MyFirstRobot_1.0.jar".into();
        let fake = Fake::new(&[]);
        assert_eq!(
            resolve(&same, &options(None), &fake).unwrap_err(),
            "download (HTTP 404)"
        );
        assert_eq!(fake.calls().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn errors_say_what_is_wrong() {
        // Wrong robot behind the right URL.
        let mut other = entry();
        other.id = "sample.Other".into();
        let fake = Fake::new(&[("http://list/x.jar", Ok(SAMPLE_JAR))]);
        let why = resolve(&other, &options(None), &fake).unwrap_err();
        assert_eq!(why, "mismatch (properties say sample.MyFirstRobot 1.0)");
        // Garbage is invalid; the invalid message wins over the later 404.
        let fake = Fake::new(&[("http://list/x.jar", Ok(b"<html>"))]);
        let why = resolve(&entry(), &options(None), &fake).unwrap_err();
        assert!(why.starts_with("invalid (not a valid jar"), "{why}");
        // Only downloads failing: the last reason.
        let fake = Fake::new(&[("http://list/x.jar", Err("timeout"))]);
        assert_eq!(
            resolve(&entry(), &options(None), &fake).unwrap_err(),
            "download (HTTP 404)"
        );
    }

    #[test]
    fn report_format() {
        let list = parse_list(
            "<pre>\nsample.MyFirstRobot 1.0,http://list/x.jar\nx.Y 2,http://list/none.jar\nx.Y 2,http://list/none.jar\n</pre>",
        );
        let fake = Fake::new(&[("http://list/x.jar", Ok(SAMPLE_JAR))]);
        let mut o = options(None);
        o.dry_run = true;
        let seen = Mutex::new(Vec::new());
        let report = run(&list, &o, &fake, None, &|row, n, total| {
            seen.lock().unwrap().push((row.id.clone(), n, total));
        });
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(report.counts["valid"], 1);
        assert_eq!(report.counts["failed"], 1);
        let json: Value = serde_json::from_str(&report.to_json()).unwrap();
        let row = &json["entries"][0];
        assert_eq!(row["id"], "sample.MyFirstRobot");
        assert_eq!(row["result"], "valid");
        assert_eq!(row["source"], "list");
        assert_eq!(row["weightClass"], "nano");
        assert_eq!(row["versionFromList"], true);
        assert_eq!(row["sha256"].as_str().unwrap().len(), 64);
        assert!(row["codesize"].as_u64().unwrap() > 0);
        assert_eq!(json["entries"][1]["error"], "download (HTTP 404)");
        assert_eq!(json["duplicates"][0]["line"], 4);
        assert_eq!(json["dryRun"], true);
        let summary = report.summary();
        assert!(summary.contains("failed: 1"), "{summary}");
        assert!(summary.contains("x.Y 2: download (HTTP 404)"), "{summary}");
        assert_eq!(
            default_report_name(Division::Melee, "2026-10-24"),
            "import-melee-2026-10-24.json"
        );
    }

    #[test]
    fn today_looks_like_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }

    /// Needs the owner's corpus and the network: the mirror for a sample of 1v1 entries.
    /// `cargo test -p robocode-s-rumble-client -- --ignored --nocapture corpus`
    #[test]
    #[ignore]
    fn corpus_and_mirror_sample() {
        let list = parse_list(ONE_V_ONE);
        let dir = PathBuf::from("/mnt/c/robocode/robots");
        let fetcher = HttpFetcher::new();
        let mut o = Options::new(Division::OneVsOne);
        o.dry_run = true;
        // Local only, over the whole list.
        o.local = Some(dir.clone());
        o.mirror = None;
        let nothing = Fake::new(&[]);
        let local = run(&list, &o, &nothing, None, &|_, _, _| {});
        let local_ok = local.entries.iter().filter(|e| e.result == "valid").count();
        println!(
            "local: {local_ok} of {} resolve from {}",
            list.entries.len(),
            dir.display()
        );
        println!(
            "{}",
            local
                .summary()
                .lines()
                .take(12)
                .collect::<Vec<_>>()
                .join("\n")
        );
        // Mirror for ~30 entries that are not local.
        let missing: Vec<Entry> = local
            .entries
            .iter()
            .zip(&list.entries)
            .filter(|(r, _)| r.result == "failed")
            .map(|(_, e)| e.clone())
            .step_by(7)
            .take(30)
            .collect();
        let sample = ParsedList {
            entries: missing,
            ..ParsedList::default()
        };
        o.local = None;
        o.mirror = Some(DEFAULT_MIRROR.to_string());
        // List URLs are not tried: point them at the mirror's own scheme to keep this about it.
        let mut by_mirror = sample.clone();
        for e in &mut by_mirror.entries {
            e.url = mirror_url(DEFAULT_MIRROR, &e.id, &e.version);
        }
        let report = run(&by_mirror, &o, &fetcher, None, &|_, _, _| {});
        println!("mirror sample:\n{}", report.summary());
    }
}
