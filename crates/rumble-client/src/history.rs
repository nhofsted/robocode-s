// SPDX-License-Identifier: EPL-2.0

//! The historic versions import (docs/rumble-backend.md §16.6): registers the versions of the
//! archives (strangeautomata, the archive.org dump) as **retired** versions of the bots the
//! rumble has, each with its best-guess release date, and sets the release date of the
//! versions it has. `robocode-s rumble admin import --history <versions.json>` wraps [`run`].
//!
//! The dataset is made by a separate job; its reading is tolerant: an entry needs `id`,
//! `version`, `sha256`, a jar in `files` (paths absolute or relative to the dataset's folder or
//! its parent) and a `releaseDate` (RFC 3339 or `YYYY-MM-DD`); its division comes from
//! `division`, else `wiki.pages` (the participants pages it was on), else the bot's other
//! entries (melee if they were all melee-only), else 1v1. Other fields are ignored.
//!
//! A version on a participants list today is the bot's listed current version: it is
//! imported normally (the server only sets its date if it has it). Every other version is
//! imported `historic` (404 for a bot the rumble doesn't have: `noBot`).

use crate::http::Api;
use crate::import::{Pacer, import_call, upload};
use rumble_core::Config;
use rumble_core::data::day_number;
use rumble_core::ids::Division;
use rumble_core::participants::check_entry;
use rumble_core::store::rfc3339;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One version of the dataset, as far as the import needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub id: String,
    pub version: String,
    pub sha256: String,
    /// The jar, resolved.
    pub file: PathBuf,
    /// Milliseconds since the epoch.
    pub released: u64,
    /// When it left the rumble (ms): the last participants revision listing it
    /// (`wiki.lastListed`), else the release of the bot's next version; `None` if neither is
    /// known (still listed, or the bot's newest). Never before `released`.
    pub retired: Option<u64>,
    pub division: Division,
}

/// A dataset entry that can't be imported, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unusable {
    /// 0-based index in `versions`.
    pub index: usize,
    pub id: Option<String>,
    pub version: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dataset {
    pub entries: Vec<HistoryEntry>,
    pub unusable: Vec<Unusable>,
}

/// Milliseconds since the epoch of an RFC 3339 time (`Z` or `±HH:MM`, optional fraction) or
/// of a `YYYY-MM-DD` date (midnight UTC).
pub fn parse_time(text: &str) -> Option<u64> {
    let text = text.trim();
    let day = day_number(text.get(..10)?)?;
    let mut ms = day.checked_mul(86_400_000)?;
    let rest = &text[10..];
    if !rest.is_empty() {
        let rest = rest.strip_prefix(['T', 't', ' '])?;
        let number = |s: &str| -> Option<i64> {
            (s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        };
        let (h, m, s) = (
            number(rest.get(0..2)?)?,
            number(rest.get(3..5)?)?,
            number(rest.get(6..8)?)?,
        );
        if rest.get(2..3)? != ":" || rest.get(5..6)? != ":" || h > 23 || m > 59 || s > 60 {
            return None;
        }
        ms += (h * 3600 + m * 60 + s) * 1000;
        let mut zone = &rest[8..];
        if let Some(fraction) = zone.strip_prefix('.') {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            let millis: String = fraction[..digits]
                .chars()
                .chain("000".chars())
                .take(3)
                .collect();
            ms += millis.parse::<i64>().ok()?;
            zone = &fraction[digits..];
        }
        match zone {
            "Z" | "z" | "" => {}
            _ => {
                let sign = match zone.get(..1)? {
                    "+" => 1,
                    "-" => -1,
                    _ => return None,
                };
                let (oh, om) = (number(zone.get(1..3)?)?, number(zone.get(4..6)?)?);
                if zone.len() != 6 || zone.get(3..4)? != ":" {
                    return None;
                }
                ms -= sign * (oh * 3600 + om * 60) * 1000;
            }
        }
    }
    u64::try_from(ms).ok()
}

/// The division a dataset entry names: `division`, else its `wiki.pages` (strings, or objects
/// with a `title`, `page` or `name`, or short names such as `1v1`, `melee`, `gigarumble`): a
/// melee page makes it melee unless a 1v1 one (GigaRumble is a 1v1 subset) is there too.
fn named_division(record: &Value) -> Option<Division> {
    if let Some(division) = record
        .get("division")
        .and_then(Value::as_str)
        .and_then(|d| d.parse().ok())
    {
        return Some(division);
    }
    let pages = record.get("wiki")?.get("pages")?;
    let mut names: Vec<&str> = Vec::new();
    match pages {
        Value::Array(items) => {
            for item in items {
                let name = item.as_str().or_else(|| {
                    ["title", "page", "name"]
                        .iter()
                        .find_map(|k| item.get(k).and_then(Value::as_str))
                });
                names.extend(name);
            }
        }
        Value::Object(map) => names.extend(map.keys().map(String::as_str)),
        _ => {}
    }
    let (mut one, mut melee) = (false, false);
    for name in names {
        let name = name.to_ascii_lowercase().replace('_', " ");
        if name.contains("melee") {
            melee = true;
        } else if name.contains("participants") || name == "1v1" || name.contains("giga") {
            one = true;
        }
    }
    match (one, melee) {
        (true, _) => Some(Division::OneVsOne),
        (false, true) => Some(Division::Melee),
        _ => None,
    }
}

fn resolve_file(dir: &Path, file: &str) -> PathBuf {
    let path = Path::new(file);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let near = dir.join(path);
    match dir.parent() {
        Some(parent) if !near.exists() && parent.join(path).exists() => parent.join(path),
        _ => near,
    }
}

/// Reads the dataset (`{"versions": [...]}`, or a bare array) of the file in `dir`.
pub fn parse_dataset(text: &str, dir: &Path) -> Result<Dataset, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("dataset: {e}"))?;
    let records = match &value {
        Value::Array(items) => items,
        other => other
            .get("versions")
            .and_then(Value::as_array)
            .ok_or("dataset: no \"versions\" array")?,
    };
    let mut dataset = Dataset::default();
    let mut divisions: Vec<Option<Division>> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let text = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let (id, version) = (text("id"), text("version"));
        let unusable = |reason: &str| Unusable {
            index,
            id: id.clone(),
            version: version.clone(),
            reason: reason.to_string(),
        };
        let file = record
            .get("files")
            .and_then(Value::as_array)
            .and_then(|f| f.first())
            .and_then(Value::as_str);
        let released = text("releaseDate");
        let missing = [
            (id.is_none(), "no id"),
            (version.is_none(), "no version"),
            (text("sha256").is_none(), "no sha256"),
            (file.is_none(), "no file"),
            (released.is_none(), "no release date"),
        ];
        if let Some((_, why)) = missing.iter().find(|(m, _)| *m) {
            dataset.unusable.push(unusable(why));
            continue;
        }
        let Some(released) = released.as_deref().and_then(parse_time) else {
            dataset.unusable.push(unusable("bad release date"));
            continue;
        };
        let last_listed = record
            .get("wiki")
            .and_then(|w| w.get("lastListed"))
            .and_then(Value::as_str)
            .and_then(parse_time);
        dataset.entries.push(HistoryEntry {
            id: id.clone().unwrap_or_default(),
            version: version.clone().unwrap_or_default(),
            sha256: text("sha256").unwrap_or_default().to_ascii_lowercase(),
            file: resolve_file(dir, file.unwrap_or_default()),
            released,
            retired: last_listed,
            division: Division::OneVsOne,
        });
        divisions.push(named_division(record));
    }
    // Entries without a division take the bot's: melee if all its named ones are melee.
    let mut melee_only: HashMap<&str, bool> = HashMap::new();
    for (entry, division) in dataset.entries.iter().zip(&divisions) {
        if let Some(division) = division {
            let only = melee_only.entry(entry.id.as_str()).or_insert(true);
            *only &= *division == Division::Melee;
        }
    }
    let fallback: HashMap<String, Division> = melee_only
        .into_iter()
        .map(|(id, only)| {
            let division = if only {
                Division::Melee
            } else {
                Division::OneVsOne
            };
            (id.to_string(), division)
        })
        .collect();
    for (entry, division) in dataset.entries.iter_mut().zip(divisions) {
        entry.division = division
            .or_else(|| fallback.get(&entry.id).copied())
            .unwrap_or(Division::OneVsOne);
    }
    // Versions the wiki never listed left when the bot's next one came out.
    let mut releases: HashMap<String, Vec<u64>> = HashMap::new();
    for entry in &dataset.entries {
        releases
            .entry(entry.id.clone())
            .or_default()
            .push(entry.released);
    }
    for list in releases.values_mut() {
        list.sort_unstable();
    }
    for entry in dataset.entries.iter_mut() {
        if entry.retired.is_none() {
            entry.retired = releases[&entry.id]
                .iter()
                .copied()
                .find(|&r| r > entry.released);
        }
        entry.retired = entry.retired.map(|r| r.max(entry.released));
    }
    Ok(dataset)
}

/// One row of the report.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRow {
    pub id: String,
    pub version: String,
    /// `created`, `exists`, `updated`, `divisionAdded`, `valid` (dry run), `noBot` or
    /// `failed`.
    pub result: String,
    /// Imported as a historic (retired) version; false for a listed current version.
    pub historic: bool,
    pub division: String,
    pub released: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryReport {
    pub dry_run: bool,
    pub counts: BTreeMap<String, usize>,
    pub entries: Vec<HistoryRow>,
    pub unusable: Vec<Unusable>,
}

impl HistoryReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    /// Counts per result and the failures.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} versions{}\n",
            self.entries.len(),
            if self.dry_run { " (dry run)" } else { "" }
        );
        for (result, n) in &self.counts {
            out.push_str(&format!("  {result}: {n}\n"));
        }
        let historic = self.entries.iter().filter(|e| e.historic).count();
        out.push_str(&format!("  historic: {historic}\n"));
        if !self.unusable.is_empty() {
            out.push_str(&format!("  unusable entries: {}\n", self.unusable.len()));
        }
        let failures: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.result == "failed")
            .collect();
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

/// `history-<date>.json`, the default report name.
pub fn default_report_name(date: &str) -> String {
    format!("history-{date}.json")
}

/// Settings of a run.
#[derive(Clone, Debug)]
pub struct HistoryOptions {
    pub concurrency: usize,
    pub dry_run: bool,
    pub config: Config,
}

/// The participants lists today: `(id, version)` → the first division listing it.
pub type Listed = HashMap<(String, String), Division>;

fn process(
    entry: &HistoryEntry,
    listed: &Listed,
    options: &HistoryOptions,
    api: Option<&Api>,
    pacer: &Pacer,
) -> HistoryRow {
    let current = listed.get(&(entry.id.clone(), entry.version.clone()));
    let division = current.copied().unwrap_or(entry.division);
    let mut row = HistoryRow {
        id: entry.id.clone(),
        version: entry.version.clone(),
        result: "valid".to_string(),
        historic: current.is_none(),
        division: division.as_str().to_string(),
        released: rfc3339(entry.released / 1000),
        error: None,
    };
    let fail = |mut row: HistoryRow, why: String| {
        row.result = "failed".to_string();
        row.error = Some(why);
        row
    };
    let bytes = match std::fs::read(&entry.file) {
        Ok(bytes) => bytes,
        Err(e) => return fail(row, format!("read ({}: {e})", entry.file.display())),
    };
    let (info, version_from_list) =
        match check_entry(&bytes, &entry.id, &entry.version, &options.config) {
            Ok(checked) => checked,
            Err(why) => return fail(row, why),
        };
    if info.sha256 != entry.sha256 {
        return fail(
            row,
            format!(
                "sha256 ({} in the dataset, {} read)",
                entry.sha256, info.sha256
            ),
        );
    }
    let Some(api) = api.filter(|_| !options.dry_run) else {
        return row;
    };
    if let Err(why) = upload(api, pacer, &info, &bytes) {
        return fail(row, why);
    }
    let mut body = json!({
        "bot": info.bot_id,
        "version": info.version,
        "sha256": info.sha256,
        "division": division.as_str(),
        "source": "archive",
        "released": entry.released,
    });
    if row.historic {
        body["historic"] = json!(true);
        if let Some(retired) = entry.retired {
            body["retired"] = json!(retired);
        }
    }
    if version_from_list {
        body["versionFromList"] = json!(true);
    }
    match import_call(api, pacer, &body) {
        Ok(result) => row.result = result,
        Err(why) if why == "no bot" => row.result = "noBot".to_string(),
        Err(why) => return fail(row, why),
    }
    row
}

/// Called as each entry finishes: the row, how many are done, how many there are.
pub type Progress<'a> = &'a (dyn Fn(&HistoryRow, usize, usize) + Sync);

/// Imports every usable entry of the dataset: `api` is required unless `options.dry_run`.
/// Paced and threaded like the list import, but a worker takes all of a bot's entries in turn:
/// two registrations of one bot at once make the server answer 409 `conflict` (its `vs` list
/// changed). The report lists them in dataset order.
pub fn run(
    dataset: &Dataset,
    listed: &Listed,
    options: &HistoryOptions,
    api: Option<&Api>,
    progress: Progress,
) -> HistoryReport {
    let total = dataset.entries.len();
    let pacer = Pacer::api();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let rows: Mutex<Vec<Option<HistoryRow>>> = Mutex::new(vec![None; total]);
    // The entries' indices by bot, bots in order of first appearance.
    let mut bots: Vec<Vec<usize>> = Vec::new();
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    for (i, entry) in dataset.entries.iter().enumerate() {
        let group = *by_id.entry(entry.id.as_str()).or_insert_with(|| {
            bots.push(Vec::new());
            bots.len() - 1
        });
        bots[group].push(i);
    }
    let workers = options.concurrency.clamp(1, 64).min(bots.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let b = next.fetch_add(1, Ordering::SeqCst);
                    let Some(group) = bots.get(b) else {
                        break;
                    };
                    for &i in group {
                        let entry = &dataset.entries[i];
                        let mut row = process(entry, listed, options, api, &pacer);
                        if api.is_none() && !options.dry_run && row.result == "valid" {
                            row.result = "failed".to_string();
                            row.error = Some("api (no API client)".to_string());
                        }
                        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                        progress(&row, n, total);
                        rows.lock().unwrap()[i] = Some(row);
                    }
                }
            });
        }
    });
    let entries: Vec<HistoryRow> = rows.into_inner().unwrap().into_iter().flatten().collect();
    let mut counts = BTreeMap::new();
    for e in &entries {
        *counts.entry(e.result.clone()).or_insert(0) += 1;
    }
    HistoryReport {
        dry_run: options.dry_run,
        counts,
        entries,
        unusable: dataset.unusable.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JAR: &[u8] =
        include_bytes!("../../rumble-core/tests/fixtures/samples/sample.MyFirstRobot.jar");

    #[test]
    fn times() {
        assert_eq!(parse_time("1970-01-01"), Some(0));
        assert_eq!(parse_time("1970-01-02T00:00:01Z"), Some(86_401_000));
        assert_eq!(parse_time("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_time("1970-01-01T00:00:00.25Z"), Some(250));
        assert_eq!(parse_time("2026-10-24T12:00:00Z"), Some(1_792_843_200_000));
        assert_eq!(parse_time("1969-12-31"), None);
        assert_eq!(parse_time("2026-10-24T25:00:00Z"), None);
        assert_eq!(parse_time("2026-10-24T12:00:00 nonsense"), None);
        assert_eq!(parse_time("soon"), None);
    }

    #[test]
    fn a_tolerant_dataset() {
        let text = r#"{"versions": [
            {"id": "a.B", "version": "1", "sha256": "AB", "files": ["jars/a.B_1.jar"],
             "releaseDate": "2005-01-02T03:04:05Z", "wiki": {"pages": ["RoboRumble/Participants/Melee"]},
             "dateSource": "x", "confidence": 0.5, "flags": []},
            {"id": "a.B", "version": "2", "sha256": "cd", "files": ["/abs/a.B_2.jar"],
             "releaseDate": "2006-01-01", "wiki": null},
            {"id": "c.D", "version": "1", "sha256": "ef", "files": ["c.jar"],
             "releaseDate": "2007-01-01", "wiki": {"pages": [{"title": "RoboRumble/Participants"},
             {"title": "RoboRumble/Participants/Melee"}]}},
            {"id": "m.N", "version": "1", "sha256": "ef", "files": ["m.jar"],
             "releaseDate": "2007-01-01", "wiki": {"pages": ["1v1", "melee"]}},
            {"id": "o.P", "version": "1", "sha256": "ef", "files": ["o.jar"],
             "releaseDate": "2007-01-01", "wiki": {"pages": ["gigarumble", "meleetop30"]}},
            {"id": "e.F", "version": "1", "sha256": "ef", "files": ["e.jar"], "releaseDate": null},
            {"id": "g.H", "version": "1", "sha256": "ef", "files": [], "releaseDate": "2007-01-01"},
            {"id": "i.J", "version": "1", "sha256": "ef", "files": ["i.jar"], "releaseDate": "2007-01-01",
             "division": "melee"},
            {"id": "k.L", "version": "1", "sha256": "ef", "files": ["k.jar"], "releaseDate": "June"}
        ]}"#;
        let dataset = parse_dataset(text, Path::new("/data/set")).unwrap();
        let rows: Vec<(&str, &str, Division, &str)> = dataset
            .entries
            .iter()
            .map(|e| {
                (
                    e.id.as_str(),
                    e.version.as_str(),
                    e.division,
                    e.file.to_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("a.B", "1", Division::Melee, "/data/set/jars/a.B_1.jar"),
                // No pages: the bot's other entries are melee-only.
                ("a.B", "2", Division::Melee, "/abs/a.B_2.jar"),
                ("c.D", "1", Division::OneVsOne, "/data/set/c.jar"),
                ("m.N", "1", Division::OneVsOne, "/data/set/m.jar"),
                ("o.P", "1", Division::OneVsOne, "/data/set/o.jar"),
                ("i.J", "1", Division::Melee, "/data/set/i.jar"),
            ]
        );
        assert_eq!(dataset.entries[0].sha256, "ab");
        // a.B 1 (2005) left when a.B 2 came out (2006); a.B 2 is the newest: still in.
        assert_eq!(dataset.entries[0].retired, parse_time("2006-01-01"));
        assert_eq!(dataset.entries[1].retired, None);
        assert_eq!(
            dataset.entries[1].released,
            parse_time("2006-01-01").unwrap()
        );
        let reasons: Vec<&str> = dataset.unusable.iter().map(|u| u.reason.as_str()).collect();
        assert_eq!(reasons, ["no release date", "no file", "bad release date"]);
        assert!(
            parse_dataset("[]", Path::new("/"))
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(parse_dataset("{}", Path::new("/")).is_err());
    }

    #[test]
    fn dry_run_checks_the_jars() {
        let dir = std::env::temp_dir().join(format!("rumble-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s.jar"), SAMPLE_JAR).unwrap();
        let sha = rumble_core::jar::sha256_hex(SAMPLE_JAR);
        let text = json!({"versions": [
            {"id": "sample.MyFirstRobot", "version": "1.0", "sha256": sha, "files": ["s.jar"],
             "releaseDate": "2001-01-01"},
            {"id": "sample.Other", "version": "0.9", "sha256": sha, "files": ["s.jar"],
             "releaseDate": "2000-01-01"},
            {"id": "sample.MyFirstRobot", "version": "1.0", "sha256": "00", "files": ["s.jar"],
             "releaseDate": "2001-01-01"},
            {"id": "x.Y", "version": "1", "sha256": "00", "files": ["missing.jar"],
             "releaseDate": "2001-01-01"}
        ]})
        .to_string();
        let dataset = parse_dataset(&text, &dir).unwrap();
        let listed: Listed = [(
            ("sample.MyFirstRobot".to_string(), "1.0".to_string()),
            Division::Melee,
        )]
        .into();
        let options = HistoryOptions {
            concurrency: 2,
            dry_run: true,
            config: Config::default(),
        };
        let report = run(&dataset, &listed, &options, None, &|_, _, _| {});
        let rows: Vec<(&str, bool, &str)> = report
            .entries
            .iter()
            .map(|r| (r.result.as_str(), r.historic, r.division.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                // The listed current version: not historic, its list's division.
                ("valid", false, "melee"),
                ("failed", true, "1v1"),
                ("failed", false, "melee"),
                ("failed", true, "1v1"),
            ]
        );
        let errors: Vec<&str> = report
            .entries
            .iter()
            .filter_map(|r| r.error.as_deref())
            .collect();
        assert!(errors[0].starts_with("mismatch"), "{errors:?}");
        assert!(errors[1].starts_with("sha256"), "{errors:?}");
        assert!(errors[2].starts_with("read"), "{errors:?}");
        assert_eq!(report.entries[0].released, "2001-01-01T00:00:00Z");
        assert!(report.summary().contains("failed: 3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
