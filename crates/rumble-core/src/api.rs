// SPDX-License-Identifier: EPL-2.0

//! Request and response bodies of the HTTP API (spec §5). JSON is `camelCase`; 64-bit seeds
//! are strings. Unknown fields are accepted everywhere: within v1 fields are only added.

use crate::ids::{Division, WeightClass};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A signed 64-bit seed, as a decimal string on the wire.
pub mod seed {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(seed: &i64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(seed)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The error codes of the §5.2 table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    EngineMismatch,
    Expired,
    Conflict,
    TooLarge,
    ClientOutdated,
    RateLimited,
    Busy,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 11] = [
        ErrorCode::BadRequest,
        ErrorCode::Unauthorized,
        ErrorCode::Forbidden,
        ErrorCode::NotFound,
        ErrorCode::EngineMismatch,
        ErrorCode::Expired,
        ErrorCode::Conflict,
        ErrorCode::TooLarge,
        ErrorCode::ClientOutdated,
        ErrorCode::RateLimited,
        ErrorCode::Busy,
    ];

    /// The `error` string of the body.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::EngineMismatch => "engine_mismatch",
            ErrorCode::Expired => "expired",
            ErrorCode::Conflict => "conflict",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::ClientOutdated => "client_outdated",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::Busy => "busy",
        }
    }

    pub fn parse(text: &str) -> Option<ErrorCode> {
        ErrorCode::ALL
            .into_iter()
            .find(|code| code.as_str() == text)
    }

    /// The HTTP status.
    pub fn status(self) -> u16 {
        match self {
            ErrorCode::BadRequest => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::EngineMismatch | ErrorCode::Expired | ErrorCode::Conflict => 409,
            ErrorCode::TooLarge => 413,
            ErrorCode::ClientOutdated => 426,
            ErrorCode::RateLimited => 429,
            ErrorCode::Busy => 503,
        }
    }
}

/// The body of every error response. `error` stays a string so that codes added later
/// still parse; [`ErrorBody::code`] maps the ones this version knows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ErrorBody {
        ErrorBody {
            error: code.as_str().to_string(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> Option<ErrorCode> {
        ErrorCode::parse(&self.error)
    }
}

/// An error response as the server's logic produces it: the code (and so the status) and a
/// message for humans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ApiError {
        ApiError {
            code,
            message: message.into(),
        }
    }

    pub fn body(&self) -> ErrorBody {
        ErrorBody::new(self.code, self.message.clone())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ApiError {}

// ---- GET info ----

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Battlefield {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InfoDivision {
    pub id: Division,
    pub rounds: u32,
    pub battlefield: Battlefield,
    pub robots: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub api: u32,
    pub engine_version: u32,
    pub min_client: String,
    pub paused: bool,
    pub github_client_id: String,
    pub deterministic_rules: u32,
    pub divisions: Vec<InfoDivision>,
    pub downloads: String,
}

// ---- GET work ----

/// The query of `GET work`.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkQuery {
    pub engine: u32,
    pub max: u32,
    pub divisions: Vec<Division>,
    pub slots: u32,
}

impl WorkQuery {
    /// The query string without the leading `?`.
    pub fn to_query(&self) -> String {
        let divisions: Vec<&str> = self.divisions.iter().map(|d| d.as_str()).collect();
        format!(
            "engine={}&max={}&divisions={}&slots={}",
            self.engine,
            self.max,
            divisions.join(","),
            self.slots
        )
    }

    /// Parses a query string (`slots` optional, default 1; `divisions` optional, default all).
    pub fn parse(query: &str) -> Result<WorkQuery, String> {
        let (mut engine, mut max, mut divisions, mut slots) = (None, None, None, 1);
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let number = |name: &str| {
                value
                    .parse::<u32>()
                    .map_err(|_| format!("{name} must be a non-negative integer"))
            };
            match key {
                "engine" => engine = Some(number("engine")?),
                "max" => max = Some(number("max")?),
                "slots" => slots = number("slots")?,
                "divisions" => {
                    let list = value
                        .split(',')
                        .filter(|d| !d.is_empty())
                        .map(str::parse)
                        .collect::<Result<Vec<Division>, _>>()?;
                    divisions = Some(list);
                }
                _ => {}
            }
        }
        let max = max.ok_or("max is required")?;
        if max == 0 {
            return Err("max must be at least 1".to_string());
        }
        Ok(WorkQuery {
            engine: engine.ok_or("engine is required")?,
            max,
            divisions: divisions.unwrap_or_else(|| Division::ALL.to_vec()),
            slots,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BattleKind {
    Normal,
    Verify,
    Probation,
    Watch,
}

/// The rules of one battle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    pub rounds: u32,
    pub width: u32,
    pub height: u32,
    pub gun_cooling_rate: f64,
    pub inactivity_time: u32,
    pub sentry_border: u32,
    pub hide_enemy_names: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkBot {
    pub id: String,
    pub version: String,
    pub vid: u32,
    pub sha256: String,
    pub size: u64,
    pub url: String,
    /// Reserved for server-held data snapshots (spec §5.4): always `null` in M4. A client
    /// refuses a battle whose `data` it doesn't understand.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkBattle {
    pub index: u32,
    pub kind: BattleKind,
    pub division: Division,
    #[serde(with = "seed")]
    pub seed: i64,
    pub rules: Rules,
    /// In the battle's robot order.
    pub bots: Vec<WorkBot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkResponse {
    pub batch: String,
    pub batch_id: String,
    pub engine_version: u32,
    pub deadline: String,
    pub battles: Vec<WorkBattle>,
}

// ---- POST results ----

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub release: String,
    pub os: String,
    pub arch: String,
    pub sandbox: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BattleStatus {
    Ok,
    RobotFailed,
    NotRun,
    /// A client-side failure (no JRE, broken sandbox): not counted.
    Error,
}

/// The colours a robot gave its tank with `setColors` and the like, each `#rrggbb` (lower
/// case); the parts it didn't colour are absent. Clients report them with a battle's
/// results ([`RobotResult::colors`]), the backend keeps a bot version's once a second run
/// of a battle confirmed them, and the data files publish them (`BotColors` in
/// `web/src/lib/rumble/types.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotColors {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gun: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radar: Option<String>,
}

impl BotColors {
    /// The colours of a protocol ARGB value (the alpha is dropped).
    pub fn hex(argb: i32) -> String {
        format!("#{:06x}", argb as u32 & 0xff_ffff)
    }

    /// These colours if they are some and all `#rrggbb`: what a client sends is checked
    /// before it is kept.
    pub fn checked(&self) -> Option<&BotColors> {
        let ok = |part: &Option<String>| {
            part.as_deref().is_none_or(|c| {
                c.len() == 7
                    && c.starts_with('#')
                    && c[1..]
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        let some = self.body.is_some() || self.gun.is_some() || self.radar.is_some();
        (some && ok(&self.body) && ok(&self.gun) && ok(&self.radar)).then_some(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RobotResult {
    pub score: f64,
    pub survival: f64,
    pub last_survivor_bonus: f64,
    pub bullet_damage: f64,
    pub bullet_damage_bonus: f64,
    pub ram_damage: f64,
    pub ram_damage_bonus: f64,
    pub firsts: u32,
    pub seconds: u32,
    pub thirds: u32,
    pub rank: u32,
    /// The colours the robot set ([`BotColors`]), as of the first snapshot in which it had
    /// set any. Absent where it set none, and from clients (or robot hosts) that don't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<BotColors>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackClaim {
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoundResult {
    pub turns: u32,
    /// The robot whose `firsts` rose in the round; -1 if none did, or several (a tie).
    pub winner: i32,
    /// Every robot whose `firsts` rose in the round. Classic gives robots that die together a
    /// first each, so a round can have more than one. Absent from older clients: `winner` then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub firsts: Vec<u32>,
    pub scores: Vec<f64>,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackClaim>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureReason {
    /// The robot couldn't be described or never connected.
    Load,
    /// Its process ended during the battle.
    Crash,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub robot: u32,
    pub reason: FailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One battle of a results post. Which optional fields are present depends on `status`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BattleResult {
    pub index: u32,
    pub status: BattleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battle_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robots: Option<Vec<RobotResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<Vec<RoundResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultsRequest {
    pub batch: String,
    pub engine_version: u32,
    pub client: ClientInfo,
    pub battles: Vec<BattleResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rejected {
    pub index: u32,
    pub error: String,
    pub message: String,
}

/// A presigned PUT for a recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UploadSlot {
    pub slot: String,
    pub index: u32,
    pub round: u32,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub expires: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    New,
    Verifier,
    Quarantined,
    Banned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserSummary {
    pub battles: u64,
    pub reputation: i64,
    pub tier: Tier,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultsResponse {
    pub accepted: Vec<u32>,
    pub rejected: Vec<Rejected>,
    pub duplicate: bool,
    pub uploads: Vec<UploadSlot>,
    pub user: UserSummary,
}

// ---- POST recordings/done ----

/// Slots per `recordings/done`; a batch can claim more, so clients confirm in chunks.
pub const RECORDINGS_DONE_MAX_SLOTS: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordingsDoneRequest {
    pub batch: String,
    pub slots: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlotFailure {
    Missing,
    Checksum,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FailedSlot {
    pub slot: String,
    pub error: SlotFailure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordingsDoneResponse {
    pub filled: Vec<String>,
    pub failed: Vec<FailedSlot>,
}

// ---- GET me, POST auth/token ----

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub uid: String,
    pub login: String,
    pub name: String,
    pub country: String,
    pub reputation: i64,
    pub tier: Tier,
    pub verify_rate: f64,
    pub battles: u64,
    pub verified_matches: u64,
    pub mismatches: u64,
    pub admin: bool,
    pub since: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthTokenRequest {
    pub github_token: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthTokenResponse {
    /// `rsu_` plus 43 base64url characters; shown once.
    pub token: String,
    pub user: Me,
}

// ---- POST bots/upload-url, POST bots/submit ----

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UploadUrlRequest {
    pub sha256: String,
    pub size: u64,
}

/// A presigned PUT, or (`admin/jars/upload-url` only) `{"exists": true}` when the jar is
/// already in the jars bucket and needs no upload (§16.2); the other fields are then empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UploadUrlResponse {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expires: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exists: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmitSource {
    Upload,
    Url,
}

/// `sha256` goes with source `upload`, `url` with source `url`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitRequest {
    pub source: SubmitSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub divisions: Vec<Division>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResponse {
    pub bot: String,
    pub version: String,
    pub vid: u32,
    /// The version's state, e.g. `probation`.
    pub state: String,
    pub codesize: u32,
    pub weight_class: WeightClass,
}

// ---- POST me, GET me/tokens, POST me/tokens/revoke, GET me/bots ----

/// `POST me`: the fields to change (absent ones stay). `country` is an ISO 3166-1 alpha-2
/// code in lower case, one of the RoboWiki's own flags (`un`, `ancap`, §16.7), or `""` to
/// clear it; `hidden` hides the user from the contributors
/// list (§15).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// One upload token or session of `GET me/tokens`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    /// The first 12 hex digits of the token's hash: what `me/tokens/revoke` takes.
    pub id: String,
    /// `upload` or `session`.
    pub kind: String,
    pub label: String,
    pub created: String,
    pub last_used: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// The token of this request.
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokensResponse {
    pub tokens: Vec<TokenInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevokeRequest {
    pub id: String,
}

/// The body of calls that have nothing else to say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: bool,
}

/// A version in `GET me/bots`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotVersionInfo {
    pub vid: u32,
    pub version: String,
    /// `probation`, `active`, `failed`, `retired`, `removed`, `delisted`.
    pub state: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codesize: Option<u32>,
    pub weight_class: WeightClass,
    pub divisions: Vec<Division>,
    pub created: String,
    pub probation_battles: u32,
    pub probation_failures: u32,
}

/// A bot in `GET me/bots`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotInfo {
    pub id: String,
    /// `active`, `retired`, `removed`, `delisted`.
    pub state: String,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub divisions: Vec<Division>,
    /// The ranked version's vid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<u32>,
    /// Newest first.
    pub versions: Vec<BotVersionInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotsResponse {
    pub bots: Vec<BotInfo>,
}

/// `POST bots/retire`: `{"bot": "id"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotRequest {
    pub bot: String,
}

/// The answer of `bots/retire`, `admin/bots/claim` and `admin/bots/remove`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotStateResponse {
    pub bot: String,
    /// The bot's state afterwards.
    pub state: String,
    /// The owner afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

// ---- admin ----

/// `POST admin/bots/import` (§16.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub bot: String,
    pub version: String,
    pub sha256: String,
    pub division: Division,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// `list`, `mirror` or `local`: informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The jar's properties have no `robot.version`; the list's `version` is authoritative
    /// (the server validates with `jar::validate_with_version`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub version_from_list: bool,
    /// The release date, milliseconds since the epoch (§16.6): set on a new version, and on
    /// an existing one whose date differs (`updated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<u64>,
    /// A version from the archives (§16.6): the bot must exist (404 otherwise); a new version
    /// is registered `retired`, behind the bot's other versions, and nothing else changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub historic: bool,
    /// With `historic`: when the version left the rumble, milliseconds since the epoch; set
    /// as `ret` on a new historic version (else its release date), and on an existing retired
    /// one whose `ret` differs (`updated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired: Option<u64>,
    /// Only set the release date `released` of the registered version of this name
    /// (§16.6), whatever its jar: no jar is read or registered; 404 if there is no such
    /// version. For a listed version, whose jar the archives may have in another build.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub date_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportResult {
    Created,
    Exists,
    DivisionAdded,
    /// The version existed with the same jar; only its release date changed.
    Updated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportResponse {
    pub vid: u32,
    pub result: ImportResult,
}

/// `POST admin/bots/claim`: `github` is a GitHub login or a user id (`g<id>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub bot: String,
    pub github: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoveRequest {
    pub bot: String,
    pub reason: String,
}

/// `POST admin/users/update`: absent fields stay as they are. `state` is `new`, `quar` or
/// `ban`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UserUpdateRequest {
    pub uid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rep: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertInfo {
    /// The alert's sort key.
    pub id: String,
    pub kind: String,
    pub text: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertsResponse {
    pub alerts: Vec<AlertInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    /// Numbers compare by value: `1100` and `1100.0` are the same JSON number.
    fn same(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
            }
            _ => a == b,
        }
    }

    /// Parses the example, serializes it again and compares the two as JSON values.
    fn round_trip<T: Serialize + DeserializeOwned>(example: &str) -> T {
        let original: Value = serde_json::from_str(example).unwrap();
        let parsed: T = serde_json::from_value(original.clone()).unwrap();
        let again = serde_json::to_value(&parsed).unwrap();
        assert!(same(&original, &again), "{original}\n!=\n{again}");
        parsed
    }

    const INFO: &str = r#"{"api": 1, "engineVersion": 1, "minClient": "0.2.0", "paused": false,
 "githubClientId": "Ov23li...", "deterministicRules": 1,
 "divisions": [{"id": "1v1", "rounds": 35, "battlefield": {"width": 800, "height": 600}, "robots": 2},
               {"id": "melee", "rounds": 35, "battlefield": {"width": 1000, "height": 1000}, "robots": 10}],
 "downloads": "https://rumble.planethofstede.be/download"}"#;

    const WORK: &str = r#"{"batch": "<batch token>", "batchId": "b-01JB2...", "engineVersion": 1,
 "deadline": "2026-10-24T15:00:00Z",
 "battles": [
  {"index": 0, "kind": "normal", "division": "1v1", "seed": "-4512334012341234",
   "rules": {"rounds": 35, "width": 800, "height": 600, "gunCoolingRate": 0.1,
             "inactivityTime": 450, "sentryBorder": 100, "hideEnemyNames": false},
   "bots": [{"id": "jk.mega.DrussGT", "version": "3.1.16", "vid": 812,
             "sha256": "9f1c...", "size": 210345, "url": "/jars/9f1c....jar", "data": null},
            {"id": "voidious.Diamond", "version": "1.8.28", "vid": 77, "sha256": "...", "size": 98123, "url": "/jars/....jar", "data": null}]}
 ]}"#;

    const RESULTS: &str = r#"{"batch": "<batch token>", "engineVersion": 1,
 "client": {"release": "0.2.0", "os": "linux", "arch": "x86_64", "sandbox": "os"},
 "battles": [
  {"index": 0, "status": "ok", "durationMs": 41230,
   "battleHash": "<hex sha256>",
   "robots": [{"score": 4123.5, "survival": 1100, "lastSurvivorBonus": 220, "bulletDamage": 2201.3,
               "bulletDamageBonus": 312.4, "ramDamage": 12.0, "ramDamageBonus": 0,
               "firsts": 22, "seconds": 13, "thirds": 0, "rank": 1},
              {"score": 3000.0, "survival": 900, "lastSurvivorBonus": 0, "bulletDamage": 1800.0,
               "bulletDamageBonus": 100.0, "ramDamage": 0, "ramDamageBonus": 0,
               "firsts": 13, "seconds": 22, "thirds": 0, "rank": 2}],
   "rounds": [{"turns": 912, "winner": 0, "scores": [151.2, 60.3],
               "hash": "<hex sha256>", "track": {"sha256": "<hex>", "size": 3401}}]},
  {"index": 1, "status": "robotFailed", "failure": {"robot": 1, "reason": "load", "detail": "class not found"}},
  {"index": 2, "status": "notRun"}
 ]}"#;

    const RESULTS_RESPONSE: &str = r#"{"accepted": [0, 1], "rejected": [{"index": 3, "error": "invalid", "message": "rounds: 34 of 35"}],
 "duplicate": false,
 "uploads": [{"slot": "1v1.812.77.a.2", "index": 0, "round": 7,
              "url": "https://<bucket>.s3.eu-west-1.amazonaws.com/rec/...?X-Amz-...",
              "headers": {"Content-Length": "3401", "x-amz-checksum-sha256": "<base64>",
                          "Content-Type": "application/octet-stream"},
              "expires": "2026-10-24T13:10:00Z"}],
 "user": {"battles": 1834, "reputation": 61, "tier": "verifier"}}"#;

    const DONE: &str = r#"{"batch": "<batch token>", "slots": ["1v1.812.77.a.2"]}"#;
    const DONE_RESPONSE: &str = r#"{"filled": ["1v1.812.77.a.2"], "failed": [{"slot": "1v1.1.2.a.0", "error": "missing"}]}"#;

    const ME: &str = r#"{"uid": "g583231", "login": "nhofsted", "name": "Nick Hofstede", "country": "be",
 "reputation": 61, "tier": "new", "verifyRate": 0.044, "battles": 1834, "verifiedMatches": 61,
 "mismatches": 0, "admin": true, "since": "2026-10-24T09:00:00Z"}"#;

    #[test]
    fn info() {
        let info: Info = round_trip(INFO);
        assert_eq!(info.divisions[1].id, Division::Melee);
    }

    #[test]
    fn work() {
        let work: WorkResponse = round_trip(WORK);
        assert_eq!(work.battles[0].seed, -4_512_334_012_341_234);
        assert_eq!(work.battles[0].kind, BattleKind::Normal);
        let json = serde_json::to_value(&work).unwrap();
        assert_eq!(json["battles"][0]["seed"], "-4512334012341234");
    }

    #[test]
    fn seeds_cover_all_of_i64() {
        let mut work: WorkResponse = serde_json::from_str(WORK).unwrap();
        for seed in [i64::MIN, i64::MAX, 0] {
            work.battles[0].seed = seed;
            let again: WorkResponse =
                serde_json::from_str(&serde_json::to_string(&work).unwrap()).unwrap();
            assert_eq!(again.battles[0].seed, seed);
        }
        let numeric = WORK.replace("\"-4512334012341234\"", "-4512334012341234");
        assert!(serde_json::from_str::<WorkResponse>(&numeric).is_err());
    }

    #[test]
    fn results() {
        let request: ResultsRequest = round_trip(RESULTS);
        assert_eq!(request.battles[1].status, BattleStatus::RobotFailed);
        assert_eq!(
            request.battles[1].failure.as_ref().unwrap().reason,
            FailureReason::Load
        );
        assert!(request.battles[2].robots.is_none());
        round_trip::<ResultsResponse>(RESULTS_RESPONSE);
    }

    #[test]
    fn bot_colors() {
        let json = RESULTS.replace(
            r#""thirds": 0, "rank": 1}"#,
            r##""thirds": 0, "rank": 1, "colors": {"body": "#000000", "gun": "#ff0000"}}"##,
        );
        let request: ResultsRequest = round_trip(&json);
        let robots = request.battles[0].robots.as_ref().unwrap();
        let colors = robots[0].colors.as_ref().unwrap();
        assert_eq!(colors.body.as_deref(), Some("#000000"));
        assert_eq!(colors.radar, None);
        assert_eq!(colors.checked(), Some(colors));
        assert_eq!(robots[1].colors, None);
        assert_eq!(BotColors::hex(0xFF29_298C_u32 as i32), "#29298c");
        assert_eq!(BotColors::hex(-1), "#ffffff");
        let radar = |c: &str| BotColors {
            radar: Some(c.into()),
            ..Default::default()
        };
        assert!(radar("#a0b1c2").checked().is_some());
        for c in ["#A0B1C2", "a0b1c2", "#a0b1c", "#a0b1c2d", "#g0b1c2", ""] {
            assert_eq!(radar(c).checked(), None, "{c}");
        }
        assert_eq!(BotColors::default().checked(), None);
    }

    #[test]
    fn recordings_done() {
        round_trip::<RecordingsDoneRequest>(DONE);
        let response: RecordingsDoneResponse = round_trip(DONE_RESPONSE);
        assert_eq!(response.failed[0].error, SlotFailure::Missing);
    }

    #[test]
    fn me_and_auth_token() {
        let me: Me = round_trip(ME);
        assert_eq!(me.tier, Tier::New);
        round_trip::<AuthTokenRequest>(r#"{"githubToken": "gho_x", "label": "nick-desktop"}"#);
        let response = format!(r#"{{"token": "rsu_abc", "user": {ME}}}"#);
        round_trip::<AuthTokenResponse>(&response);
    }

    #[test]
    fn bots() {
        round_trip::<UploadUrlRequest>(r#"{"sha256": "ab", "size": 123456}"#);
        round_trip::<UploadUrlResponse>(
            r#"{"url": "https://s3/x", "headers": {"Content-Length": "5"}, "expires": "2026-10-24T13:10:00Z"}"#,
        );
        let upload: SubmitRequest = round_trip(
            r#"{"source": "upload", "sha256": "ab", "divisions": ["1v1", "melee"], "homepage": "https://x"}"#,
        );
        assert_eq!(upload.source, SubmitSource::Upload);
        let url: SubmitRequest =
            round_trip(r#"{"source": "url", "url": "https://x/y.jar", "divisions": ["1v1"]}"#);
        assert_eq!(url.homepage, None);
        let response: SubmitResponse = round_trip(
            r#"{"bot": "id", "version": "v", "vid": 913, "state": "probation", "codesize": 1234, "weightClass": "mini"}"#,
        );
        assert_eq!(response.weight_class, WeightClass::Mini);
    }

    #[test]
    fn unknown_fields_are_accepted() {
        let info = INFO.replacen('{', r#"{"added": {"later": 1}, "#, 1);
        assert!(serde_json::from_str::<Info>(&info).is_ok());
        let work = WORK.replace(r#""index": 0,"#, r#""index": 0, "extra": [1],"#);
        assert!(serde_json::from_str::<WorkResponse>(&work).is_ok());
        let results = RESULTS.replace(
            r#""release": "0.2.0","#,
            r#""release": "0.2.0", "gpu": "x","#,
        );
        assert!(serde_json::from_str::<ResultsRequest>(&results).is_ok());
        let me = ME.replacen('{', r#"{"extra": true, "#, 1);
        assert!(serde_json::from_str::<Me>(&me).is_ok());
    }

    #[test]
    fn error_codes() {
        let body = ErrorBody::new(ErrorCode::EngineMismatch, "engine 2 is current");
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"error": "engine_mismatch", "message": "engine 2 is current"})
        );
        assert_eq!(body.code(), Some(ErrorCode::EngineMismatch));
        assert_eq!(body.code().unwrap().status(), 409);
        let statuses: Vec<u16> = ErrorCode::ALL.iter().map(|c| c.status()).collect();
        assert_eq!(
            statuses,
            [400, 401, 403, 404, 409, 409, 409, 413, 426, 429, 503]
        );
        for code in ErrorCode::ALL {
            assert_eq!(ErrorCode::parse(code.as_str()), Some(code));
        }
        let future: ErrorBody =
            serde_json::from_str(r#"{"error": "teapot", "message": "x", "extra": 1}"#).unwrap();
        assert_eq!(future.code(), None);
    }

    #[test]
    fn work_query() {
        let query = WorkQuery {
            engine: 1,
            max: 20,
            divisions: vec![Division::OneVsOne, Division::Melee],
            slots: 4,
        };
        assert_eq!(
            query.to_query(),
            "engine=1&max=20&divisions=1v1,melee&slots=4"
        );
        assert_eq!(WorkQuery::parse(&query.to_query()), Ok(query));
        let minimal = WorkQuery::parse("engine=2&max=5").unwrap();
        assert_eq!(minimal.divisions, Division::ALL.to_vec());
        assert_eq!(minimal.slots, 1);
        assert!(WorkQuery::parse("max=5").is_err());
        assert!(WorkQuery::parse("engine=1&max=0").is_err());
        assert!(WorkQuery::parse("engine=1&max=5&divisions=teams").is_err());
    }
}
