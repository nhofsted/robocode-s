// SPDX-License-Identifier: EPL-2.0

//! Configuration (spec §3): every tunable with its default, and the merge of the overrides
//! kept in `state/config.json`.

use crate::ids::Division;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The engine version handed out by default (the Lambdas override it with their build's).
pub const DEFAULT_ENGINE_VERSION: u32 = 1;

/// The deterministic rules of the robot hosts the rumble ranks
/// (`robocode_s_server::DETERMINISTIC_RULES`, which the Lambdas don't link). `GET info`
/// reports it; a test in `rumble-client` keeps the two equal, as it does for the engine
/// version.
pub const DETERMINISTIC_RULES: u32 = 1;

/// All tunables. Every field has a default, so a partial JSON object deserializes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub engine_version: u32,
    pub min_client: String,
    pub paused: bool,
    pub divisions: PerDivision<DivisionConfig>,
    pub weight_limits: WeightLimits,
    pub batch: BatchConfig,
    pub rate: RateConfig,
    pub rep: RepConfig,
    pub probation: ProbationConfig,
    pub slots: SlotsConfig,
    pub plan: PlanConfig,
    pub jar: JarConfig,
    pub submit: SubmitConfig,
    pub sync: SyncConfig,
    /// GitHub numeric ids with the admin role.
    pub admins: Vec<u64>,
}

/// One value per division.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerDivision<T> {
    #[serde(rename = "1v1")]
    pub one_vs_one: T,
    pub melee: T,
}

impl<T> PerDivision<T> {
    pub fn get(&self, division: Division) -> &T {
        match division {
            Division::OneVsOne => &self.one_vs_one,
            Division::Melee => &self.melee,
        }
    }
}

/// Battle rules and share of one division.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DivisionConfig {
    pub rounds: u32,
    pub width: u32,
    pub height: u32,
    /// Robots per battle.
    pub robots: u32,
    /// Share of the battles planned for the division.
    pub share: f64,
    pub gun_cooling_rate: f64,
    pub inactivity_time: u32,
    pub sentry_border: u32,
    pub hide_enemy_names: bool,
}

impl DivisionConfig {
    fn with(rounds: u32, width: u32, height: u32, robots: u32, share: f64) -> DivisionConfig {
        DivisionConfig {
            rounds,
            width,
            height,
            robots,
            share,
            gun_cooling_rate: 0.1,
            inactivity_time: 450,
            sentry_border: 100,
            hide_enemy_names: false,
        }
    }
}

impl Default for DivisionConfig {
    fn default() -> Self {
        DivisionConfig::with(35, 800, 600, 2, 0.7)
    }
}

/// Codesize limits: a bot is in a class if its codesize is **below** the limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WeightLimits {
    pub nano: u32,
    pub micro: u32,
    pub mini: u32,
}

impl Default for WeightLimits {
    fn default() -> Self {
        WeightLimits {
            nano: 250,
            micro: 750,
            mini: 1500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BatchConfig {
    /// Most battles per work request.
    pub max_battles: PerDivision<u32>,
    pub deadline_minutes: u32,
}

impl Default for BatchConfig {
    fn default() -> Self {
        BatchConfig {
            max_battles: PerDivision {
                one_vs_one: 50,
                melee: 10,
            },
            deadline_minutes: 180,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RateConfig {
    pub work_seconds: u32,
    pub results_seconds: u32,
}

impl Default for RateConfig {
    fn default() -> Self {
        RateConfig {
            work_seconds: 10,
            results_seconds: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepConfig {
    pub start_rate: f64,
    pub floor_rate: f64,
    pub tau: f64,
    pub verifier_min: u32,
    pub recorder_min: u32,
    pub quarantine_matches: u32,
    pub recheck_on_penalty: u32,
    pub ban_mismatches: u32,
    pub ban_window_days: u32,
    /// Three-way disagreements after which a version is flagged nondeterministic (§11.5);
    /// 0 turns the automatic flag off.
    pub nd_disagreements: u32,
}

impl Default for RepConfig {
    fn default() -> Self {
        RepConfig {
            start_rate: 0.20,
            floor_rate: 0.01,
            tau: 40.0,
            verifier_min: 50,
            recorder_min: 20,
            quarantine_matches: 20,
            recheck_on_penalty: 20,
            ban_mismatches: 3,
            ban_window_days: 90,
            nd_disagreements: 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProbationConfig {
    pub battles: u32,
    pub fail_share: f64,
}

impl Default for ProbationConfig {
    fn default() -> Self {
        ProbationConfig {
            battles: 10,
            fail_share: 0.8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlotsConfig {
    pub top: u32,
    pub per_side: u32,
    pub max_bytes: u64,
}

impl Default for SlotsConfig {
    fn default() -> Self {
        SlotsConfig {
            top: 50,
            per_side: 5,
            max_bytes: 262_144,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanConfig {
    /// Prior variance of a battle's pairing score (APS points squared).
    pub prior_variance: PerDivision<f64>,
    pub prior_weight: f64,
    /// Prior variance of a bot's pairing means over its opponents (APS points squared): how
    /// uncertain an APS from a few opponents is. Its weight is `prior_weight`.
    pub opponent_variance: PerDivision<f64>,
    pub close_factor: f64,
    pub rank_factor: f64,
    pub min_hourly: PerDivision<u32>,
    pub headroom: f64,
}

impl Default for PlanConfig {
    fn default() -> Self {
        PlanConfig {
            prior_variance: PerDivision {
                one_vs_one: 16.0,
                melee: 25.0,
            },
            prior_weight: 3.0,
            opponent_variance: PerDivision {
                one_vs_one: 625.0,
                melee: 121.0,
            },
            close_factor: 10.0,
            rank_factor: 2.0,
            min_hourly: PerDivision {
                one_vs_one: 200,
                melee: 20,
            },
            headroom: 1.5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JarConfig {
    /// Compressed size limit.
    pub max_bytes: u64,
    /// Zip-bomb guard: total unpacked bytes and entries.
    pub max_unpacked: u64,
    pub max_entries: u32,
}

impl Default for JarConfig {
    fn default() -> Self {
        JarConfig {
            max_bytes: 5 << 20,
            max_unpacked: 20 << 20,
            max_entries: 5000,
        }
    }
}

/// Submission limits (spec §12.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubmitConfig {
    /// Bots one user may own.
    pub max_bots: u32,
    /// New versions per bot per day.
    pub versions_per_day: u32,
    /// Younger GitHub accounts need the owner's approval to submit.
    pub min_account_days: u32,
}

impl Default for SubmitConfig {
    fn default() -> Self {
        SubmitConfig {
            max_bots: 5,
            versions_per_day: 5,
            min_account_days: 30,
        }
    }
}

/// The daily LiteRumble sync (spec §16.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncConfig {
    pub enabled: bool,
    /// The participants pages (`action=raw`).
    pub lists: PerDivision<String>,
    /// URL template with `{id}` and `{version}`, tried after the list's URL; empty: none.
    pub mirror: String,
    /// A list that parses to fewer entries than this share of the last good sync's (or to
    /// fewer than `minEntries`) is not trusted: nothing changes in its division.
    pub min_list_share: f64,
    pub min_entries: u32,
    /// More versions to delist than this in one run: none are delisted (an alert instead).
    pub max_delist_per_run: u32,
    /// New versions downloaded per run; the rest wait for the next day.
    pub max_imports_per_run: u32,
    /// The country flags page (§16.7, `action=raw`); empty: not read.
    pub flags: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            enabled: true,
            lists: PerDivision {
                one_vs_one: crate::participants::LIST_1V1.to_string(),
                melee: crate::participants::LIST_MELEE.to_string(),
            },
            mirror: crate::participants::DEFAULT_MIRROR.to_string(),
            min_list_share: 0.9,
            min_entries: 100,
            max_delist_per_run: 50,
            max_imports_per_run: 200,
            flags: crate::flags::FLAGS_PAGE.to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            engine_version: DEFAULT_ENGINE_VERSION,
            min_client: "0.2.0".to_string(),
            paused: false,
            divisions: PerDivision {
                one_vs_one: DivisionConfig::with(35, 800, 600, 2, 0.7),
                melee: DivisionConfig::with(35, 1000, 1000, 10, 0.3),
            },
            weight_limits: WeightLimits::default(),
            batch: BatchConfig::default(),
            rate: RateConfig::default(),
            rep: RepConfig::default(),
            probation: ProbationConfig::default(),
            slots: SlotsConfig::default(),
            plan: PlanConfig::default(),
            jar: JarConfig::default(),
            submit: SubmitConfig::default(),
            sync: SyncConfig::default(),
            admins: vec![4_036_752],
        }
    }
}

/// Merges `overrides` over `base`: objects key by key, anything else replaces.
fn merge(base: &mut Value, overrides: &Value) {
    match (base, overrides) {
        (Value::Object(base), Value::Object(overrides)) => {
            for (key, value) in overrides {
                match base.get_mut(key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, overrides) => *base = overrides.clone(),
    }
}

impl Config {
    /// The defaults with a partial JSON object merged over them (nested objects merge key
    /// by key; arrays and scalars replace). Unknown keys are ignored.
    pub fn with_overrides(overrides: &Value) -> Result<Config, String> {
        if !overrides.is_object() {
            return Err("config overrides must be a JSON object".to_string());
        }
        let mut value = serde_json::to_value(Config::default()).map_err(|e| e.to_string())?;
        merge(&mut value, overrides);
        serde_json::from_value(value).map_err(|e| format!("invalid config: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_the_spec() {
        let config = Config::default();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value,
            json!({
                "engineVersion": 1,
                "minClient": "0.2.0",
                "paused": false,
                "divisions": {
                    "1v1": {"rounds": 35, "width": 800, "height": 600, "robots": 2, "share": 0.7,
                            "gunCoolingRate": 0.1, "inactivityTime": 450, "sentryBorder": 100,
                            "hideEnemyNames": false},
                    "melee": {"rounds": 35, "width": 1000, "height": 1000, "robots": 10, "share": 0.3,
                              "gunCoolingRate": 0.1, "inactivityTime": 450, "sentryBorder": 100,
                              "hideEnemyNames": false}
                },
                "weightLimits": {"nano": 250, "micro": 750, "mini": 1500},
                "batch": {"maxBattles": {"1v1": 50, "melee": 10}, "deadlineMinutes": 180},
                "rate": {"workSeconds": 10, "resultsSeconds": 5},
                "rep": {"startRate": 0.2, "floorRate": 0.01, "tau": 40.0, "verifierMin": 50,
                        "recorderMin": 20, "quarantineMatches": 20, "recheckOnPenalty": 20,
                        "banMismatches": 3, "banWindowDays": 90, "ndDisagreements": 3},
                "probation": {"battles": 10, "failShare": 0.8},
                "slots": {"top": 50, "perSide": 5, "maxBytes": 262144},
                "plan": {"priorVariance": {"1v1": 16.0, "melee": 25.0}, "priorWeight": 3.0,
                         "opponentVariance": {"1v1": 625.0, "melee": 121.0},
                         "closeFactor": 10.0, "rankFactor": 2.0,
                         "minHourly": {"1v1": 200, "melee": 20}, "headroom": 1.5},
                "jar": {"maxBytes": 5242880, "maxUnpacked": 20971520, "maxEntries": 5000},
                "submit": {"maxBots": 5, "versionsPerDay": 5, "minAccountDays": 30},
                "sync": {"enabled": true,
                         "lists": {"1v1": "https://robowiki.net/w/index.php?title=RoboRumble/Participants&action=raw",
                                   "melee": "https://robowiki.net/w/index.php?title=RoboRumble/Participants/Melee&action=raw"},
                         "mirror": "https://robocode-archive.strangeautomata.com/robots/{id}_{version}.jar",
                         "minListShare": 0.9, "minEntries": 100, "maxDelistPerRun": 50,
                         "maxImportsPerRun": 200,
                         "flags": "https://robowiki.net/w/index.php?title=RoboRumble/Country_Flags&action=raw"},
                "admins": [4036752]
            })
        );
        assert_eq!(config.divisions.get(Division::Melee).robots, 10);
    }

    #[test]
    fn submit_limits() {
        let config = Config::with_overrides(&json!({"submit": {"maxBots": 8}})).unwrap();
        assert_eq!(config.submit.max_bots, 8);
        assert_eq!(config.submit.versions_per_day, 5);
        assert_eq!(config.submit.min_account_days, 30);
    }

    #[test]
    fn empty_overrides_are_the_defaults() {
        assert_eq!(
            Config::with_overrides(&json!({})).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn overrides_merge_key_by_key() {
        let config = Config::with_overrides(&json!({
            "paused": true,
            "engineVersion": 2,
            "divisions": {"melee": {"rounds": 20}},
            "batch": {"maxBattles": {"1v1": 5}},
            "admins": [1, 2],
            "futureKey": 7
        }))
        .unwrap();
        let defaults = Config::default();
        assert!(config.paused);
        assert_eq!(config.engine_version, 2);
        assert_eq!(config.divisions.melee.rounds, 20);
        assert_eq!(config.divisions.melee.width, 1000);
        assert_eq!(config.divisions.one_vs_one, defaults.divisions.one_vs_one);
        assert_eq!(config.batch.max_battles.one_vs_one, 5);
        assert_eq!(config.batch.max_battles.melee, 10);
        assert_eq!(config.batch.deadline_minutes, 180);
        assert_eq!(config.admins, vec![1, 2]);
        assert_eq!(config.rep, defaults.rep);
    }

    #[test]
    fn bad_overrides_are_errors() {
        assert!(Config::with_overrides(&json!([1])).is_err());
        assert!(Config::with_overrides(&json!({"paused": "yes"})).is_err());
        assert!(Config::with_overrides(&json!({"rate": 3})).is_err());
    }

    #[test]
    fn partial_config_deserializes() {
        let config: Config = serde_json::from_str(r#"{"rate": {"workSeconds": 30}}"#).unwrap();
        assert_eq!(config.rate.work_seconds, 30);
        assert_eq!(config.rate.results_seconds, 5);
    }
}
