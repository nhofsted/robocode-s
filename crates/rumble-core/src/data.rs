// SPDX-License-Identifier: EPL-2.0

//! The rumble data files (docs/rumble-data.md, spec §15): serde types that mirror
//! `web/src/lib/rumble/types.ts` field for field, which is the source of truth for their
//! semantics. Nullable fields serialize as `null` (never absent); percentages are 0-100,
//! timestamps ISO 8601 UTC strings, dates `YYYY-MM-DD`.
//!
//! Division and rumble ids are plain strings here: the files also carry the `teams` and
//! `twinduel` divisions, which `ids::Division` does not know yet.

use crate::ids::WeightClass;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The colours a bot version's robot gives its tank (`BotColors` in `types.ts`): published
/// where the site draws tanks, absent until the rumble knows them and for robots that set
/// none (the site then derives colours from the bot id).
pub use crate::api::BotColors;

/// The `schema` of every file, and the `/data/v1` URL prefix.
pub const SCHEMA_VERSION: u32 = 1;

/// Daily history points this far back (today included); weekly before that.
pub const DAILY_DAYS: i64 = 365;

/// `/data/v1/rumbles.json`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RumbleIndex {
    pub schema: u32,
    pub generated_at: String,
    pub rumbles: Vec<RumbleSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RumbleSummary {
    /// Rumble id: the division, plus `-<class>` for the weight-class rumbles.
    pub id: String,
    pub division: String,
    pub weight_class: WeightClass,
    /// Bots currently ranked.
    pub bots: u32,
    /// Battles that count towards the current rankings.
    pub battles: u64,
    pub rounds: u32,
    pub battlefield: Battlefield,
    pub engine_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battlefield {
    pub width: u32,
    pub height: u32,
}

/// The ranking metrics, all derived from the pairing statistics.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub aps: f64,
    pub pwin: f64,
    pub survival: f64,
    pub glicko: f64,
    pub glicko_rd: f64,
    pub anpp: f64,
    pub vote: f64,
}

/// `/data/v1/rankings/<rumble>.json`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rankings {
    pub schema: u32,
    pub rumble: String,
    pub generated_at: String,
    pub engine_version: u32,
    /// Sorted by APS, best first.
    pub bots: Vec<RankingRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingRow {
    pub id: String,
    pub version: String,
    pub author: String,
    pub country: Option<String>,
    /// Rank by APS, 1-based.
    pub rank: u32,
    /// Opponents this bot has fought at least once.
    pub pairings: u32,
    pub battles: u64,
    /// Rank by APS one week ago; null for bots that were not ranked then.
    pub rank_week_ago: Option<u32>,
    /// The ranked version's colours (additive; absent where unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<BotColors>,
    #[serde(flatten)]
    pub metrics: Metrics,
}

/// `/data/v1/bots/<id>.json`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotDetails {
    pub schema: u32,
    pub id: String,
    pub author: Author,
    pub description: Option<String>,
    pub homepage: Option<String>,
    /// Newest first; the first entry is the current version.
    pub versions: Vec<BotVersion>,
    /// Only the rumbles this bot takes part in.
    pub rumbles: BTreeMap<String, BotRumble>,
    /// Recorded rounds of the current version by division (`1v1`, `melee`): the bot has
    /// `/data/v1/rec/<division>/<id>.json` where this is above 0. Absent: none (M6).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recordings: BTreeMap<String, u32>,
    /// `active`, `retired` or `delisted` (additive; absent in files written before it): a
    /// delisted bot left the RoboWiki participants lists and is no longer in the rumble.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Author {
    pub name: String,
    /// The author's GitHub login, once they have claimed the bot.
    pub github: Option<String>,
    /// ISO 3166-1 alpha-2, lower case.
    pub country: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotVersion {
    pub version: String,
    /// Codesize in bytes (Java robots), null where not defined.
    pub codesize: Option<u32>,
    pub weight_class: WeightClass,
    /// SHA-256 of the robot package, hex.
    pub sha256: String,
    /// Size of the jar in bytes (files written before this field existed read as 0).
    #[serde(default)]
    pub size: u64,
    pub added_at: String,
    pub retired_at: Option<String>,
    /// The version's colours (additive; absent where unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<BotColors>,
    /// The release date as well as it is known (additive; absent where unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotRumble {
    pub rank: u32,
    pub battles: u64,
    pub pairings: Vec<Pairing>,
    /// Oldest first, across all versions (thinned, see `thin_history`).
    pub history: Vec<HistoryPoint>,
    #[serde(flatten)]
    pub metrics: Metrics,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub opponent: String,
    pub opponent_version: String,
    pub battles: u32,
    /// This bot's average percent score against the opponent.
    pub score: f64,
    /// Average percentage of rounds this bot survived.
    pub survival: f64,
    pub normalized: f64,
    /// Problem-bot index (KNN-PBI).
    pub pbi: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryPoint {
    pub date: String,
    pub version: String,
    pub rank: u32,
    pub aps: f64,
    pub glicko: f64,
}

/// `/data/v1/contributors.json` (spec §15 step 6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributors {
    pub schema: u32,
    pub generated_at: String,
    pub engine_version: u32,
    /// Sorted by battles, most first.
    pub contributors: Vec<Contributor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributor {
    pub login: String,
    pub country: Option<String>,
    pub battles: u64,
    /// Battles in the last 7 days.
    pub battles_week: u64,
    /// `new`, `verifier` or `quarantined`.
    pub tier: String,
    /// When the volunteer first signed in.
    pub since: String,
}

/// `/data/v1/archive.json`: every downloadable version of every bot that has not been taken
/// down, for the archive page. Sorted by bot id, each bot's versions newest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Archive {
    pub schema: u32,
    pub generated_at: String,
    pub bots: Vec<ArchiveBot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchiveBot {
    pub id: String,
    pub author: String,
    pub versions: Vec<ArchiveVersion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchiveVersion {
    pub version: String,
    /// The jar is `/jars/<sha256>.jar`.
    pub sha256: String,
    pub size: u64,
    /// `active`, `probation`, `retired`, `failed` or `delisted`.
    pub state: String,
    /// `YYYY-MM-DD`.
    pub added: String,
    /// The release date, `YYYY-MM-DD` (additive; absent where unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date; `None` if it is not one.
pub fn day_number(date: &str) -> Option<i64> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let part = |range: std::ops::Range<usize>| -> Option<i64> {
        let text = &date[range];
        text.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| text.parse().ok())
            .flatten()
    };
    let (year, month, day) = (part(0..4)?, part(5..7)?, part(8..10)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil (H. Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Thins a history (oldest first) as of `today` (spec §15 step 5): every point of the last
/// `DAILY_DAYS` days is kept; before that only the days whose day number since 1970-01-01 is
/// divisible by 7. Points whose date does not parse are dropped.
pub fn thin_history(history: Vec<HistoryPoint>, today: &str) -> Vec<HistoryPoint> {
    let Some(today) = day_number(today) else {
        return history;
    };
    history
        .into_iter()
        .filter(|point| match day_number(&point.date) {
            Some(day) => today - day < DAILY_DAYS || day.rem_euclid(7) == 0,
            None => false,
        })
        .collect()
}

/// Appends today's point to a history (oldest first), replacing a point of the same day (a
/// later run on the same day wins), then thins it as of the new point's date.
pub fn push_history(mut history: Vec<HistoryPoint>, point: HistoryPoint) -> Vec<HistoryPoint> {
    history.retain(|existing| existing.date != point.date);
    let today = point.date.clone();
    history.push(point);
    thin_history(history, &today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn point(date: &str) -> HistoryPoint {
        HistoryPoint {
            date: date.to_string(),
            version: "1.0".to_string(),
            rank: 1,
            aps: 50.0,
            glicko: 1500.0,
        }
    }

    fn date_of(day: i64) -> String {
        // Days to civil (H. Hinnant), the inverse of `day_number`.
        let z = day + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}")
    }

    #[test]
    fn day_numbers() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("1970-01-08"), Some(7));
        assert_eq!(day_number("2000-03-01"), Some(11_017));
        assert_eq!(day_number("2026-10-24"), Some(20_750));
        assert_eq!(day_number("1969-12-31"), Some(-1));
        assert_eq!(day_number("2026-13-01"), None);
        assert_eq!(day_number("2026-1-01"), None);
        assert_eq!(day_number("20261001"), None);
        assert_eq!(day_number("2026-1a-01"), None);
        assert_eq!(date_of(20_750), "2026-10-24");
        assert_eq!(date_of(20_750 - 365), "2025-10-24");
        assert_eq!(date_of(0), "1970-01-01");
    }

    #[test]
    fn thinning_keeps_a_year_daily_then_weekly() {
        let today = day_number("2026-10-24").unwrap();
        let history: Vec<HistoryPoint> =
            (today - 800..=today).map(|d| point(&date_of(d))).collect();
        let thinned = thin_history(history, "2026-10-24");
        // 365 daily points (today and the 364 days before), plus the multiples of 7 before.
        let old: Vec<_> = thinned
            .iter()
            .filter(|p| today - day_number(&p.date).unwrap() >= DAILY_DAYS)
            .collect();
        assert!(
            old.iter()
                .all(|p| day_number(&p.date).unwrap().rem_euclid(7) == 0)
        );
        let expected_old = (today - 800..=today - DAILY_DAYS)
            .filter(|d| d.rem_euclid(7) == 0)
            .count();
        assert_eq!(old.len(), expected_old);
        assert_eq!(thinned.len() - old.len(), 365);
        // Order is preserved.
        assert!(thinned.windows(2).all(|w| w[0].date < w[1].date));
        // Day 20,385 - 364 is the oldest daily point, whatever its weekday.
        assert!(thinned.iter().any(|p| p.date == date_of(today - 364)));
    }

    #[test]
    fn thinning_boundary() {
        let today = day_number("2026-10-24").unwrap();
        // A day 365 back is weekly only: keep it iff divisible by 7.
        let d365 = today - 365;
        let kept = thin_history(vec![point(&date_of(d365))], "2026-10-24");
        assert_eq!(kept.len(), usize::from(d365.rem_euclid(7) == 0));
        let kept = thin_history(vec![point(&date_of(today - 364))], "2026-10-24");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn same_day_replaces() {
        let mut first = point("2026-10-24");
        first.rank = 5;
        let history = push_history(vec![point("2026-10-23")], first);
        let mut later = point("2026-10-24");
        later.rank = 3;
        let history = push_history(history, later);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].rank, 3);
        assert_eq!(history[0].date, "2026-10-23");
    }

    #[test]
    fn pushing_thins_old_points() {
        // A weekly-kept and a dropped date well before the daily window.
        let n = day_number("2026-10-24").unwrap() - 400;
        let weekly = n + (7 - n.rem_euclid(7)) % 7;
        let keep = date_of(weekly);
        let dropped = date_of(weekly + 1);
        let history = push_history(vec![point(&keep), point(&dropped)], point("2026-10-24"));
        let dates: Vec<_> = history.iter().map(|p| p.date.as_str()).collect();
        assert_eq!(dates, [keep.as_str(), "2026-10-24"]);
    }

    #[test]
    fn nulls_serialize_as_null_and_metrics_flatten() {
        let row = RankingRow {
            id: "a.B".into(),
            version: "1".into(),
            author: "x".into(),
            country: None,
            rank: 1,
            pairings: 2,
            battles: 3,
            rank_week_ago: None,
            colors: None,
            metrics: Metrics {
                aps: 50.0,
                pwin: 50.0,
                survival: 50.0,
                glicko: 1500.0,
                glicko_rd: 100.0,
                anpp: 50.0,
                vote: 1.0,
            },
        };
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["country"], Value::Null);
        assert_eq!(value["rankWeekAgo"], Value::Null);
        assert_eq!(value["glickoRd"], json!(100.0));
        // Colours are additive: absent where unknown, an object where known.
        assert!(value.get("colors").is_none());
        assert_eq!(serde_json::from_value::<RankingRow>(value).unwrap(), row);
        let colored = RankingRow {
            colors: Some(BotColors {
                body: Some("#000000".into()),
                gun: Some("#ff0000".into()),
                radar: None,
            }),
            ..row
        };
        let value = serde_json::to_value(&colored).unwrap();
        assert_eq!(
            value["colors"],
            json!({"body": "#000000", "gun": "#ff0000"})
        );
        assert_eq!(
            serde_json::from_value::<RankingRow>(value).unwrap(),
            colored
        );
    }

    // The files of web/src/lib/rumble/fixtures.ts (generated by `node web/tools/sample-data.ts`
    // and cut down), through the Rust types and back.

    /// Numbers compare as f64: JS writes 50 where serde writes 50.0.
    fn normalize(value: Value) -> Value {
        match value {
            Value::Number(n) => json!(n.as_f64().unwrap()),
            Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
            }
            other => other,
        }
    }

    fn round_trip<T: Serialize + for<'de> Deserialize<'de>>(path: &str) {
        let full = format!("{}/tests/fixtures/data/{path}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{full}: {e}"));
        let original: Value = serde_json::from_str(&text).unwrap();
        let typed: T =
            serde_json::from_value(original.clone()).unwrap_or_else(|e| panic!("{path}: {e}"));
        let again = serde_json::to_value(&typed).unwrap();
        assert_eq!(normalize(again), normalize(original), "{path}");
    }

    #[test]
    fn fixture_files_round_trip() {
        round_trip::<RumbleIndex>("rumbles.json");
        round_trip::<Contributors>("contributors.json");
        round_trip::<Archive>("archive.json");
        round_trip::<Rankings>("rankings/1v1.json");
        round_trip::<Rankings>("rankings/twinduel.json");
        round_trip::<BotDetails>("bots/yb.lab.SlyTortoise.json");
        round_trip::<BotDetails>("bots/zc.ThunderMoth.json");
    }
}
