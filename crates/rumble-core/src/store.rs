// SPDX-License-Identifier: EPL-2.0

//! The `Store` trait: the DynamoDB items of spec §9.2 and the operations of §9.3, as the rest
//! of the core needs them, and [`MemStore`], an in-memory implementation for tests that
//! enforces the same conditions.
//!
//! **Synchronous on purpose.** The core has no async (spec §2), and every caller here is a
//! short sequence of item reads and conditional writes. The DynamoDB implementation (P7)
//! implements this trait 1:1 and runs the core's entry points (`apply_results`, `resolve`,
//! ...) on `tokio::task::spawn_blocking`, where each method may call
//! `Handle::block_on(client.op().send())`. (`async fn` in traits would be available with
//! `rust-version` 1.85, but it would make the whole core async and its tests need an
//! executor, for no gain in a Lambda that serves one request at a time.)
//!
//! Each method is one DynamoDB request (GetItem, PutItem, UpdateItem with a condition,
//! DeleteItem, Query), except where its documentation names a transaction. A condition that
//! doesn't hold is [`StoreError::ConditionFailed`] (DynamoDB's
//! `ConditionalCheckFailedException`, or a `TransactionCanceledException` whose reason is
//! `ConditionalCheckFailed`), so callers can make writes idempotent and resolve races.
//!
//! TTL attributes are carried on the items; [`MemStore`] keeps expired items (DynamoDB's TTL
//! deletion is lazy too, so callers never rely on it).

use crate::api::{ApiError, BotColors, ErrorCode, Rejected};
use crate::ids::{Division, WeightClass};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

/// Seconds in a day.
pub const DAY: u64 = 86_400;
/// Result log items live 400 days (§9.2).
pub const LOG_TTL: u64 = 400 * DAY;
/// Batch markers live 7 days past the batch's deadline.
pub const MARKER_TTL: u64 = 7 * DAY;
/// Verification items live 30 days.
pub const VERIFY_TTL: u64 = 30 * DAY;
/// Alerts live 90 days.
pub const ALERT_TTL: u64 = 90 * DAY;
/// The `uid` of correction records (§11.3).
pub const SYSTEM_UID: &str = "system";

/// Why a store operation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The write's condition didn't hold; nothing was written.
    ConditionFailed,
    /// Throttled after the SDK's retries (503 `busy`).
    Busy,
    /// Anything else (network, malformed item, ...).
    Other(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::ConditionFailed => f.write_str("condition failed"),
            StoreError::Busy => f.write_str("store busy"),
            StoreError::Other(why) => write!(f, "store error: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> ApiError {
        match error {
            StoreError::ConditionFailed => {
                ApiError::new(ErrorCode::Conflict, "a concurrent change; retry")
            }
            StoreError::Busy | StoreError::Other(_) => {
                ApiError::new(ErrorCode::Busy, "the server is busy; retry later")
            }
        }
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

// ---- users ----

/// A user's state (`st`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UserState {
    #[default]
    New,
    Quarantined,
    Banned,
}

impl UserState {
    pub fn as_str(self) -> &'static str {
        match self {
            UserState::New => "new",
            UserState::Quarantined => "quar",
            UserState::Banned => "ban",
        }
    }

    pub fn parse(text: &str) -> Result<UserState, String> {
        match text {
            "new" => Ok(UserState::New),
            "quar" => Ok(UserState::Quarantined),
            "ban" => Ok(UserState::Banned),
            _ => Err(format!("unknown user state {text:?}")),
        }
    }
}

/// The reputation attributes of a user. They change by read-modify-write, guarded by `ver`
/// (a new attribute, not in §9.2's table): [`Store::set_reputation`] writes them only if
/// `ver` is unchanged. The counters written by the results transaction (`bat`, `lp`) are
/// separate attributes, so the two never overwrite each other.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reputation {
    /// `ver`: incremented by every [`Store::set_reputation`].
    pub ver: u64,
    /// `rep`: matches since the last reset.
    pub rep: u32,
    /// `st`.
    pub state: UserState,
    /// `vm`: verified matches.
    pub verified_matches: u64,
    /// `mm`: confirmed mismatches.
    pub mismatches: u64,
    /// `mmt`: times (unix s) of the mismatches within the ban window.
    pub mismatch_times: Vec<u64>,
    /// `qm`: matches while quarantined.
    pub quarantine_matches: u32,
}

/// The user item (`U#<uid>`, `P`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct User {
    pub uid: String,
    pub login: String,
    pub name: String,
    pub country: String,
    /// `ghid`.
    pub github_id: u64,
    /// `ghc`: the GitHub account's creation time (unix s), from the profile at sign-in.
    pub github_created: Option<u64>,
    /// `adm`.
    pub admin: bool,
    /// `tr`: the owner lifted the account-age limit (§12.2).
    pub trusted: bool,
    /// `hid`: hidden from the contributors list.
    pub hidden: bool,
    /// `bo` (new, SS): the bot ids the user owns, kept by submission and claims, so the
    /// `submit.maxBots` limit needs no query (the table has no index by owner).
    pub bots: Vec<String>,
    /// `bat`: battles reported and counted.
    pub battles: u64,
    /// `lp`: last results post (unix s).
    pub last_post: Option<u64>,
    /// `cr` (unix s).
    pub created: u64,
    pub reputation: Reputation,
}

/// Profile and admin changes of a user; `None` leaves an attribute as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserPatch {
    pub login: Option<String>,
    pub name: Option<String>,
    pub country: Option<String>,
    pub github_created: Option<u64>,
    pub admin: Option<bool>,
    pub trusted: Option<bool>,
    pub hidden: Option<bool>,
    pub bots: Option<Vec<String>>,
}

impl UserPatch {
    /// Applies the patch to an in-memory user.
    pub fn apply(&self, user: &mut User) {
        if let Some(login) = &self.login {
            user.login = login.clone();
        }
        if let Some(name) = &self.name {
            user.name = name.clone();
        }
        if let Some(country) = &self.country {
            user.country = country.clone();
        }
        if let Some(created) = self.github_created {
            user.github_created = Some(created);
        }
        if let Some(admin) = self.admin {
            user.admin = admin;
        }
        if let Some(trusted) = self.trusted {
            user.trusted = trusted;
        }
        if let Some(hidden) = self.hidden {
            user.hidden = hidden;
        }
        if let Some(bots) = &self.bots {
            user.bots = bots.clone();
        }
    }
}

// ---- tokens ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Upload,
    Session,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Upload => "upload",
            TokenKind::Session => "session",
        }
    }

    pub fn parse(text: &str) -> Result<TokenKind, String> {
        match text {
            "upload" => Ok(TokenKind::Upload),
            "session" => Ok(TokenKind::Session),
            _ => Err(format!("unknown token kind {text:?}")),
        }
    }
}

/// A token or session: the item `T#<hash>`/`T` and its index item `U#<uid>`/`T#<hash12>`
/// (the same attributes; `id` is the index's 12 hex digits).
#[derive(Clone, Debug, PartialEq)]
pub struct TokenItem {
    /// SHA-256 hex of the token.
    pub hash: String,
    pub uid: String,
    pub kind: TokenKind,
    /// `lbl`.
    pub label: String,
    /// `cr`.
    pub created: u64,
    /// `lu`: last used, written at most daily.
    pub last_used: u64,
    /// The TTL: sessions only.
    pub expires: Option<u64>,
}

impl TokenItem {
    /// The id shown to the user and used by `me/tokens/revoke`: the first 12 hex digits.
    pub fn id(&self) -> &str {
        &self.hash[..self.hash.len().min(12)]
    }
}

// ---- bots and versions ----

/// A bot's state (`st`). `delisted` (owner, 2026-10-26): an unclaimed imported bot that left
/// the RoboWiki participants lists; its current version is delisted too (`cur` still points to
/// it), and it is active again when it is listed again (§16.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotState {
    Active,
    Retired,
    Removed,
    Delisted,
}

impl BotState {
    pub fn as_str(self) -> &'static str {
        match self {
            BotState::Active => "active",
            BotState::Retired => "retired",
            BotState::Removed => "removed",
            BotState::Delisted => "delisted",
        }
    }

    pub fn parse(text: &str) -> Result<BotState, String> {
        match text {
            "active" => Ok(BotState::Active),
            "retired" => Ok(BotState::Retired),
            "removed" => Ok(BotState::Removed),
            "delisted" => Ok(BotState::Delisted),
            _ => Err(format!("unknown bot state {text:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotSource {
    Import,
    Submit,
}

impl BotSource {
    pub fn as_str(self) -> &'static str {
        match self {
            BotSource::Import => "import",
            BotSource::Submit => "submit",
        }
    }

    pub fn parse(text: &str) -> Result<BotSource, String> {
        match text {
            "import" => Ok(BotSource::Import),
            "submit" => Ok(BotSource::Submit),
            _ => Err(format!("unknown bot source {text:?}")),
        }
    }
}

/// The bot item (`BOT#<id>`, `M`).
#[derive(Clone, Debug, PartialEq)]
pub struct BotItem {
    pub id: String,
    /// `au`.
    pub author: String,
    /// `uid`: the owner, if claimed or submitted.
    pub owner: Option<String>,
    pub homepage: Option<String>,
    pub description: Option<String>,
    pub divisions: Vec<Division>,
    /// `vs`: newest first.
    pub versions: Vec<u32>,
    /// `cur`: the ranked version.
    pub current: Option<u32>,
    pub state: BotState,
    pub created: u64,
    pub source: BotSource,
}

/// A version's state (`st`). `delisted` (§16.5): the version of an unclaimed imported bot
/// that left the participants lists. Neither planned nor ranked, but not dead either: its
/// pairings stay in the state (frozen), its recordings, slot items and jar stay, and it is
/// active again, with those pairings, when the same version is listed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionState {
    Probation,
    Active,
    Failed,
    Retired,
    Removed,
    Delisted,
}

impl VersionState {
    pub const ALL: [VersionState; 6] = [
        VersionState::Probation,
        VersionState::Active,
        VersionState::Failed,
        VersionState::Retired,
        VersionState::Removed,
        VersionState::Delisted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VersionState::Probation => "probation",
            VersionState::Active => "active",
            VersionState::Failed => "failed",
            VersionState::Retired => "retired",
            VersionState::Removed => "removed",
            VersionState::Delisted => "delisted",
        }
    }

    pub fn parse(text: &str) -> Result<VersionState, String> {
        VersionState::ALL
            .into_iter()
            .find(|state| state.as_str() == text)
            .ok_or_else(|| format!("unknown version state {text:?}"))
    }
}

/// The version item (`REG`, `V#<vid36>`).
#[derive(Clone, Debug, PartialEq)]
pub struct VersionItem {
    pub vid: u32,
    pub bot: String,
    pub version: String,
    pub sha256: String,
    pub size: u64,
    pub codesize: Option<u32>,
    pub class: WeightClass,
    pub divisions: Vec<Division>,
    pub state: VersionState,
    /// `pb`, `pf`.
    pub probation_battles: u32,
    pub probation_failures: u32,
    /// `cls`: the main class.
    pub class_name: String,
    pub created: u64,
    pub retired: Option<u64>,
    pub url: Option<String>,
    /// `col` (JSON): the colours the robot sets, once two runs of a battle agreed on them
    /// ([`Store::set_version_colors`]); `None` until then, and for robots that set none.
    pub colors: Option<BotColors>,
    /// `rd` (new): the release date, **milliseconds** since the epoch, as well as it is known:
    /// the daily sync's first sighting on a participants list, or the archives' date
    /// (§16.5, §16.6). `None` for versions registered before.
    pub released: Option<u64>,
}

// ---- results ----

/// The batch marker (`B#<batchId>`, `B`): makes a results post count once (§5.4).
#[derive(Clone, Debug, PartialEq)]
pub struct BatchMarker {
    pub batch_id: String,
    pub uid: String,
    /// `acc`.
    pub accepted: Vec<u32>,
    /// `rej`.
    pub rejected: Vec<Rejected>,
    /// `slots`: claimed slot ids.
    pub slots: Vec<String>,
    /// The TTL: the batch's deadline plus [`MARKER_TTL`].
    pub ttl: u64,
}

/// A result log item (`R#<E>#<YYYYMMDDHH>`, `<post ms>#<batchId>`).
#[derive(Clone, Debug, PartialEq)]
pub struct LogItem {
    pub engine: u32,
    /// `YYYYMMDDHH` (UTC) of the post.
    pub hour: u32,
    /// The sort key.
    pub sort: String,
    pub uid: String,
    /// `recs`: concatenated records (`record::Record::encode`).
    pub records: Vec<u8>,
    /// `sd` (new, B: 8 bytes LE per record): each record's battle seed, in record order, so
    /// that a penalized user's recent battles can be re-run (§11.3 rechecks need the seed,
    /// which the record doesn't hold). 160 bytes for 20 1v1 battles: the item stays one WCU.
    pub seeds: Vec<i64>,
    /// `nb`.
    pub battles: u32,
    /// `held`: a quarantined user's records; the planner skips them.
    pub held: bool,
    pub ttl: u64,
}

/// What `POST results` writes in one `TransactWriteItems` (§9.3), all or nothing: the batch
/// marker (condition: absent), the log item (if the post has records) and the user's
/// counters (`ADD bat :battles SET lp = :lastPost`, condition: the user exists).
#[derive(Clone, Debug, PartialEq)]
pub struct ResultsTransaction {
    pub marker: BatchMarker,
    pub log: Option<LogItem>,
    pub uid: String,
    pub battles: u64,
    pub last_post: u64,
}

// ---- verification queue ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyKind {
    /// A battle picked at random (§11.2).
    Check,
    /// A new version's battle (§12.3), created by the planner.
    Probation,
    /// A penalized user's recent battle (§11.3).
    Recheck,
}

impl VerifyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyKind::Check => "check",
            VerifyKind::Probation => "probation",
            VerifyKind::Recheck => "recheck",
        }
    }

    pub fn parse(text: &str) -> Result<VerifyKind, String> {
        match text {
            "check" => Ok(VerifyKind::Check),
            "probation" => Ok(VerifyKind::Probation),
            "recheck" => Ok(VerifyKind::Recheck),
            _ => Err(format!("unknown verification kind {text:?}")),
        }
    }
}

/// One run of a verification item.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub uid: String,
    /// `h`: the battle hash; `None` for a recheck's original run (its hash isn't kept, the
    /// record is compared instead) and for a `robotFailed` run.
    pub hash: Option<String>,
    /// `rec`: the run's record, encoded.
    pub record: Vec<u8>,
    /// The record was held (a quarantined user's) and is not counted yet.
    pub held: bool,
    /// `col` (JSON, absent when empty): the colours each robot set, as the run reported
    /// them (checked, [`BotColors::checked`]), by robot; empty from clients that don't.
    pub colors: Vec<Option<BotColors>>,
}

/// A verification item (`V#<E>`, `<created ms>#<id>`).
#[derive(Clone, Debug, PartialEq)]
pub struct VerifyItem {
    pub engine: u32,
    pub sort: String,
    pub division: Division,
    pub seed: i64,
    /// Battle robot order.
    pub bots: Vec<u32>,
    pub rounds: u32,
    pub runs: Vec<Run>,
    /// Users that ran it (never served to them again).
    pub uids: Vec<String>,
    pub stage: u8,
    /// `cl`, `cu`.
    pub claimant: Option<String>,
    pub claim_until: Option<u64>,
    pub kind: VerifyKind,
    pub ttl: u64,
}

impl VerifyItem {
    /// Free to claim: unclaimed or the claim has expired.
    pub fn claimable(&self, now: u64) -> bool {
        self.claimant.is_none() || self.claim_until.is_none_or(|until| until <= now)
    }
}

// ---- slots ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotStatus {
    /// `c`
    Claimed,
    /// `f`
    Filled,
}

impl SlotStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SlotStatus::Claimed => "c",
            SlotStatus::Filled => "f",
        }
    }

    pub fn parse(text: &str) -> Result<SlotStatus, String> {
        match text {
            "c" => Ok(SlotStatus::Claimed),
            "f" => Ok(SlotStatus::Filled),
            _ => Err(format!("unknown slot status {text:?}")),
        }
    }
}

/// One slot in a slot item's map `m`.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotEntry {
    pub status: SlotStatus,
    /// `b`: the claiming batch.
    pub batch: String,
    /// `i` (new): the battle's index in the batch, for the repost's upload list.
    pub index: u32,
    /// `r`: the round.
    pub round: u32,
    /// `h`: the track's SHA-256, base64 (S3's checksum form).
    pub sha256_b64: String,
    /// `n`: the track's size.
    pub size: u64,
    /// `x`: claim expiry (unix s).
    pub claim_until: u64,
    /// `sd`.
    pub seed: i64,
    /// `v`: the battle's robot order (vids, as in `claim_battle`): robot `k` of the track is
    /// `order[k]`. Empty in entries written before M6, which `cleanup` reopens.
    pub order: Vec<u32>,
}

/// A slot item (`S#<E>#<division>#<key>`, `S`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotItem {
    pub entries: BTreeMap<String, SlotEntry>,
    pub full: bool,
}

/// An alert (`ALERT`, `<ms>#<id>`).
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub sort: String,
    pub kind: String,
    pub text: String,
    pub reference: String,
    pub ttl: u64,
}

/// The table, as the core uses it. See the module documentation for the sync/async choice.
pub trait Store {
    // -- users --
    fn get_user(&self, uid: &str) -> StoreResult<Option<User>>;
    /// PutItem, condition: no user with this uid.
    fn create_user(&self, user: &User) -> StoreResult<()>;
    /// UpdateItem of the patch's attributes, condition: the user exists.
    fn patch_user(&self, uid: &str, patch: &UserPatch) -> StoreResult<()>;
    /// UpdateItem of the reputation attributes with `ver = expected_ver + 1`, condition:
    /// `ver = expected_ver` (`reputation.ver` is ignored).
    fn set_reputation(
        &self,
        uid: &str,
        expected_ver: u64,
        reputation: &Reputation,
    ) -> StoreResult<()>;

    // -- tokens --
    /// Puts the token item and its index item (TransactWriteItems), condition: the hash is
    /// new.
    fn put_token(&self, token: &TokenItem) -> StoreResult<()>;
    fn get_token(&self, hash: &str) -> StoreResult<Option<TokenItem>>;
    /// Query `U#<uid>` with `begins_with(sk, "T#")`: the index items.
    fn list_tokens(&self, uid: &str) -> StoreResult<Vec<TokenItem>>;
    /// Sets `lu` and, for a renewed session, the new expiry on both items.
    fn touch_token(&self, hash: &str, last_used: u64, expires: Option<u64>) -> StoreResult<()>;
    /// Deletes both items (absent ones are no error).
    fn delete_token(&self, uid: &str, hash: &str) -> StoreResult<()>;

    // -- bots and versions --
    /// `ADD n 1` on the counter, returning the new value.
    fn next_vid(&self) -> StoreResult<u32>;
    fn get_bot(&self, id: &str) -> StoreResult<Option<BotItem>>;
    fn get_version(&self, vid: u32) -> StoreResult<Option<VersionItem>>;
    /// Query `REG`.
    fn list_versions(&self) -> StoreResult<Vec<VersionItem>>;
    /// One transaction (§12.2): Put the version (condition: absent) and Put the bot,
    /// condition: absent if `expected_versions` is `None`, else its `vs` equals it.
    fn register_version(
        &self,
        bot: &BotItem,
        expected_versions: Option<&[u32]>,
        version: &VersionItem,
    ) -> StoreResult<()>;
    /// Unconditional puts, for transitions the planner and admin commands make.
    fn put_bot(&self, bot: &BotItem) -> StoreResult<()>;
    fn put_version(&self, version: &VersionItem) -> StoreResult<()>;
    /// Put of the version, condition: the stored version's `st` is `expected` (the planner's
    /// transitions, so that a concurrent retirement or takedown is not overwritten).
    fn update_version(&self, version: &VersionItem, expected: VersionState) -> StoreResult<()>;
    /// UpdateItem `SET col`, condition: the version exists and has no colours yet (the first
    /// confirmed colours stay). A put of the version read before replaces them, which only
    /// delays them to the next confirmed battle.
    fn set_version_colors(&self, vid: u32, colors: &BotColors) -> StoreResult<()>;

    // -- results --
    fn get_marker(&self, batch_id: &str) -> StoreResult<Option<BatchMarker>>;
    /// The results transaction ([`ResultsTransaction`]). `ConditionFailed` if the marker
    /// exists (the batch was posted) or the user doesn't.
    fn commit_results(&self, transaction: &ResultsTransaction) -> StoreResult<()>;
    /// `ADD slots :ids` on the marker.
    fn add_marker_slots(&self, batch_id: &str, slots: &[String]) -> StoreResult<()>;
    /// PutItem of a log item outside a results post (releases, corrections), condition:
    /// absent.
    fn append_log(&self, item: &LogItem) -> StoreResult<()>;
    /// Query `R#<E>#<hour>`, all pages, in sort key order.
    fn query_log(&self, engine: u32, hour: u32) -> StoreResult<Vec<LogItem>>;

    // -- verification queue --
    /// PutItem, condition: absent.
    fn put_verify(&self, item: &VerifyItem) -> StoreResult<()>;
    fn get_verify(&self, engine: u32, sort: &str) -> StoreResult<Option<VerifyItem>>;
    /// Query `V#<E>` oldest first, after `after` (exclusive) if given, at most `limit` items
    /// (DynamoDB's `Limit`: no filter, callers filter).
    fn query_verify(
        &self,
        engine: u32,
        after: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<VerifyItem>>;
    /// Sets `cl = uid, cu = until`, condition: the item exists, `uid` is not in `uids`, and it
    /// is unclaimed or `cu <= now`.
    fn claim_verify(
        &self,
        engine: u32,
        sort: &str,
        uid: &str,
        until: u64,
        now: u64,
    ) -> StoreResult<()>;
    /// Replaces the item, condition: its `cl` is `claimant`.
    fn replace_verify(&self, item: &VerifyItem, claimant: &str) -> StoreResult<()>;
    /// Deletes the item, condition: it exists and, if `claimant` is given, `cl` is it.
    fn delete_verify(&self, engine: u32, sort: &str, claimant: Option<&str>) -> StoreResult<()>;

    // -- slots --
    fn get_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
    ) -> StoreResult<Option<SlotItem>>;
    /// One UpdateItem: sets each of `set`'s entries in `m` and `full`, condition: each of
    /// `expected`'s slots is exactly as given (`None`: absent). Creates the item if missing
    /// (P7: a Put with `attribute_not_exists(pk)` and `m = {}` first, since a nested SET needs
    /// the map to exist).
    fn update_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
        expected: &[(String, Option<SlotEntry>)],
        set: &[(String, SlotEntry)],
        full: bool,
    ) -> StoreResult<()>;

    /// Deletes a slot item (`cleanup`, §12.4); a missing item is no error.
    fn delete_slots(&self, engine: u32, division: Division, key: &str) -> StoreResult<()>;
    /// One UpdateItem: removes each of `remove`'s entries from `m` and sets `full` to false,
    /// condition: each is exactly as given (reopening slots, §12.5).
    fn remove_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
        remove: &[(String, SlotEntry)],
    ) -> StoreResult<()>;

    // -- alerts --
    fn put_alert(&self, alert: &Alert) -> StoreResult<()>;
    fn list_alerts(&self) -> StoreResult<Vec<Alert>>;
}

/// A shared store is a store (the Lambdas share one between the router and their tests).
impl<S: Store + ?Sized> Store for std::sync::Arc<S> {
    fn get_user(&self, uid: &str) -> StoreResult<Option<User>> {
        (**self).get_user(uid)
    }
    fn create_user(&self, user: &User) -> StoreResult<()> {
        (**self).create_user(user)
    }
    fn patch_user(&self, uid: &str, patch: &UserPatch) -> StoreResult<()> {
        (**self).patch_user(uid, patch)
    }
    fn set_reputation(&self, uid: &str, expected: u64, reputation: &Reputation) -> StoreResult<()> {
        (**self).set_reputation(uid, expected, reputation)
    }
    fn put_token(&self, token: &TokenItem) -> StoreResult<()> {
        (**self).put_token(token)
    }
    fn get_token(&self, hash: &str) -> StoreResult<Option<TokenItem>> {
        (**self).get_token(hash)
    }
    fn list_tokens(&self, uid: &str) -> StoreResult<Vec<TokenItem>> {
        (**self).list_tokens(uid)
    }
    fn touch_token(&self, hash: &str, last_used: u64, expires: Option<u64>) -> StoreResult<()> {
        (**self).touch_token(hash, last_used, expires)
    }
    fn delete_token(&self, uid: &str, hash: &str) -> StoreResult<()> {
        (**self).delete_token(uid, hash)
    }
    fn next_vid(&self) -> StoreResult<u32> {
        (**self).next_vid()
    }
    fn get_bot(&self, id: &str) -> StoreResult<Option<BotItem>> {
        (**self).get_bot(id)
    }
    fn get_version(&self, vid: u32) -> StoreResult<Option<VersionItem>> {
        (**self).get_version(vid)
    }
    fn list_versions(&self) -> StoreResult<Vec<VersionItem>> {
        (**self).list_versions()
    }
    fn register_version(
        &self,
        bot: &BotItem,
        expected_versions: Option<&[u32]>,
        version: &VersionItem,
    ) -> StoreResult<()> {
        (**self).register_version(bot, expected_versions, version)
    }
    fn put_bot(&self, bot: &BotItem) -> StoreResult<()> {
        (**self).put_bot(bot)
    }
    fn put_version(&self, version: &VersionItem) -> StoreResult<()> {
        (**self).put_version(version)
    }
    fn update_version(&self, version: &VersionItem, expected: VersionState) -> StoreResult<()> {
        (**self).update_version(version, expected)
    }
    fn set_version_colors(&self, vid: u32, colors: &BotColors) -> StoreResult<()> {
        (**self).set_version_colors(vid, colors)
    }
    fn get_marker(&self, batch_id: &str) -> StoreResult<Option<BatchMarker>> {
        (**self).get_marker(batch_id)
    }
    fn commit_results(&self, transaction: &ResultsTransaction) -> StoreResult<()> {
        (**self).commit_results(transaction)
    }
    fn add_marker_slots(&self, batch_id: &str, slots: &[String]) -> StoreResult<()> {
        (**self).add_marker_slots(batch_id, slots)
    }
    fn append_log(&self, item: &LogItem) -> StoreResult<()> {
        (**self).append_log(item)
    }
    fn query_log(&self, engine: u32, hour: u32) -> StoreResult<Vec<LogItem>> {
        (**self).query_log(engine, hour)
    }
    fn put_verify(&self, item: &VerifyItem) -> StoreResult<()> {
        (**self).put_verify(item)
    }
    fn get_verify(&self, engine: u32, sort: &str) -> StoreResult<Option<VerifyItem>> {
        (**self).get_verify(engine, sort)
    }
    fn query_verify(
        &self,
        engine: u32,
        after: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<VerifyItem>> {
        (**self).query_verify(engine, after, limit)
    }
    fn claim_verify(
        &self,
        engine: u32,
        sort: &str,
        uid: &str,
        until: u64,
        now: u64,
    ) -> StoreResult<()> {
        (**self).claim_verify(engine, sort, uid, until, now)
    }
    fn replace_verify(&self, item: &VerifyItem, claimant: &str) -> StoreResult<()> {
        (**self).replace_verify(item, claimant)
    }
    fn delete_verify(&self, engine: u32, sort: &str, claimant: Option<&str>) -> StoreResult<()> {
        (**self).delete_verify(engine, sort, claimant)
    }
    fn get_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
    ) -> StoreResult<Option<SlotItem>> {
        (**self).get_slots(engine, division, key)
    }
    fn update_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
        expected: &[(String, Option<SlotEntry>)],
        set: &[(String, SlotEntry)],
        full: bool,
    ) -> StoreResult<()> {
        (**self).update_slots(engine, division, key, expected, set, full)
    }
    fn put_alert(&self, alert: &Alert) -> StoreResult<()> {
        (**self).put_alert(alert)
    }
    fn list_alerts(&self) -> StoreResult<Vec<Alert>> {
        (**self).list_alerts()
    }
    fn delete_slots(&self, engine: u32, division: Division, key: &str) -> StoreResult<()> {
        (**self).delete_slots(engine, division, key)
    }
    fn remove_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
        remove: &[(String, SlotEntry)],
    ) -> StoreResult<()> {
        (**self).remove_slots(engine, division, key, remove)
    }
}

/// Civil date and time of a unix time (UTC): (year, month, day, hour, minute, second).
pub fn civil(unix: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (unix / DAY) as i64;
    let secs = unix % DAY;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        (secs / 3600) as u32,
        (secs % 3600 / 60) as u32,
        (secs % 60) as u32,
    )
}

/// The `YYYYMMDDHH` of a unix time (UTC), as in the result log's partition keys.
pub fn hour_key(unix: u64) -> u32 {
    let (year, month, day, hour, _, _) = civil(unix);
    year as u32 * 1_000_000 + month * 10_000 + day * 100 + hour
}

/// RFC 3339 in UTC with seconds: `2026-10-24T15:00:00Z`.
pub fn rfc3339(unix: u64) -> String {
    let (year, month, day, hour, minute, second) = civil(unix);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[derive(Default)]
struct Tables {
    users: BTreeMap<String, User>,
    tokens: BTreeMap<String, TokenItem>,
    vid_counter: u32,
    bots: BTreeMap<String, BotItem>,
    versions: BTreeMap<u32, VersionItem>,
    markers: BTreeMap<String, BatchMarker>,
    log: BTreeMap<(u32, u32, String), LogItem>,
    verify: BTreeMap<(u32, String), VerifyItem>,
    slots: BTreeMap<(u32, Division, String), SlotItem>,
    alerts: BTreeMap<String, Alert>,
}

/// An in-memory [`Store`] with the same conditions as the DynamoDB one, for tests.
#[derive(Default)]
pub struct MemStore {
    tables: Mutex<Tables>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }

    fn with<T>(&self, f: impl FnOnce(&mut Tables) -> StoreResult<T>) -> StoreResult<T> {
        let mut tables = self
            .tables
            .lock()
            .map_err(|_| StoreError::Other("poisoned".into()))?;
        f(&mut tables)
    }

    /// Every verification item of an engine, oldest first (for tests).
    pub fn verify_items(&self, engine: u32) -> Vec<VerifyItem> {
        self.query_verify(engine, None, usize::MAX).unwrap()
    }

    /// Every log item of an engine, in key order (for tests).
    pub fn log_items(&self, engine: u32) -> Vec<LogItem> {
        let tables = self.tables.lock().unwrap();
        tables
            .log
            .iter()
            .filter(|((e, _, _), _)| *e == engine)
            .map(|(_, item)| item.clone())
            .collect()
    }
}

impl Store for MemStore {
    fn get_user(&self, uid: &str) -> StoreResult<Option<User>> {
        self.with(|t| Ok(t.users.get(uid).cloned()))
    }

    fn create_user(&self, user: &User) -> StoreResult<()> {
        self.with(|t| {
            if t.users.contains_key(&user.uid) {
                return Err(StoreError::ConditionFailed);
            }
            t.users.insert(user.uid.clone(), user.clone());
            Ok(())
        })
    }

    fn patch_user(&self, uid: &str, patch: &UserPatch) -> StoreResult<()> {
        self.with(|t| {
            let user = t.users.get_mut(uid).ok_or(StoreError::ConditionFailed)?;
            patch.apply(user);
            Ok(())
        })
    }

    fn set_reputation(
        &self,
        uid: &str,
        expected_ver: u64,
        reputation: &Reputation,
    ) -> StoreResult<()> {
        self.with(|t| {
            let user = t.users.get_mut(uid).ok_or(StoreError::ConditionFailed)?;
            if user.reputation.ver != expected_ver {
                return Err(StoreError::ConditionFailed);
            }
            user.reputation = Reputation {
                ver: expected_ver + 1,
                ..reputation.clone()
            };
            Ok(())
        })
    }

    fn put_token(&self, token: &TokenItem) -> StoreResult<()> {
        self.with(|t| {
            if t.tokens.contains_key(&token.hash) {
                return Err(StoreError::ConditionFailed);
            }
            t.tokens.insert(token.hash.clone(), token.clone());
            Ok(())
        })
    }

    fn get_token(&self, hash: &str) -> StoreResult<Option<TokenItem>> {
        self.with(|t| Ok(t.tokens.get(hash).cloned()))
    }

    fn list_tokens(&self, uid: &str) -> StoreResult<Vec<TokenItem>> {
        self.with(|t| {
            let mut tokens: Vec<TokenItem> = t
                .tokens
                .values()
                .filter(|tok| tok.uid == uid)
                .cloned()
                .collect();
            tokens.sort_by(|a, b| a.id().cmp(b.id()));
            Ok(tokens)
        })
    }

    fn touch_token(&self, hash: &str, last_used: u64, expires: Option<u64>) -> StoreResult<()> {
        self.with(|t| {
            let token = t.tokens.get_mut(hash).ok_or(StoreError::ConditionFailed)?;
            token.last_used = last_used;
            if expires.is_some() {
                token.expires = expires;
            }
            Ok(())
        })
    }

    fn delete_token(&self, uid: &str, hash: &str) -> StoreResult<()> {
        self.with(|t| {
            if t.tokens.get(hash).is_some_and(|tok| tok.uid == uid) {
                t.tokens.remove(hash);
            }
            Ok(())
        })
    }

    fn next_vid(&self) -> StoreResult<u32> {
        self.with(|t| {
            t.vid_counter += 1;
            Ok(t.vid_counter)
        })
    }

    fn get_bot(&self, id: &str) -> StoreResult<Option<BotItem>> {
        self.with(|t| Ok(t.bots.get(id).cloned()))
    }

    fn get_version(&self, vid: u32) -> StoreResult<Option<VersionItem>> {
        self.with(|t| Ok(t.versions.get(&vid).cloned()))
    }

    fn list_versions(&self) -> StoreResult<Vec<VersionItem>> {
        self.with(|t| Ok(t.versions.values().cloned().collect()))
    }

    fn register_version(
        &self,
        bot: &BotItem,
        expected_versions: Option<&[u32]>,
        version: &VersionItem,
    ) -> StoreResult<()> {
        self.with(|t| {
            if t.versions.contains_key(&version.vid) {
                return Err(StoreError::ConditionFailed);
            }
            let bot_ok = match (t.bots.get(&bot.id), expected_versions) {
                (None, None) => true,
                (Some(existing), Some(expected)) => existing.versions == expected,
                _ => false,
            };
            if !bot_ok {
                return Err(StoreError::ConditionFailed);
            }
            t.versions.insert(version.vid, version.clone());
            t.bots.insert(bot.id.clone(), bot.clone());
            Ok(())
        })
    }

    fn put_bot(&self, bot: &BotItem) -> StoreResult<()> {
        self.with(|t| {
            t.bots.insert(bot.id.clone(), bot.clone());
            Ok(())
        })
    }

    fn put_version(&self, version: &VersionItem) -> StoreResult<()> {
        self.with(|t| {
            t.versions.insert(version.vid, version.clone());
            Ok(())
        })
    }

    fn update_version(&self, version: &VersionItem, expected: VersionState) -> StoreResult<()> {
        self.with(|t| {
            match t.versions.get(&version.vid) {
                Some(existing) if existing.state == expected => {}
                _ => return Err(StoreError::ConditionFailed),
            }
            t.versions.insert(version.vid, version.clone());
            Ok(())
        })
    }

    fn set_version_colors(&self, vid: u32, colors: &BotColors) -> StoreResult<()> {
        self.with(|t| match t.versions.get_mut(&vid) {
            Some(version) if version.colors.is_none() => {
                version.colors = Some(colors.clone());
                Ok(())
            }
            _ => Err(StoreError::ConditionFailed),
        })
    }

    fn get_marker(&self, batch_id: &str) -> StoreResult<Option<BatchMarker>> {
        self.with(|t| Ok(t.markers.get(batch_id).cloned()))
    }

    fn commit_results(&self, tx: &ResultsTransaction) -> StoreResult<()> {
        self.with(|t| {
            if t.markers.contains_key(&tx.marker.batch_id) || !t.users.contains_key(&tx.uid) {
                return Err(StoreError::ConditionFailed);
            }
            if let Some(log) = &tx.log {
                let key = (log.engine, log.hour, log.sort.clone());
                if t.log.contains_key(&key) {
                    return Err(StoreError::ConditionFailed);
                }
                t.log.insert(key, log.clone());
            }
            t.markers
                .insert(tx.marker.batch_id.clone(), tx.marker.clone());
            let user = t.users.get_mut(&tx.uid).expect("checked above");
            user.battles += tx.battles;
            user.last_post = Some(tx.last_post);
            Ok(())
        })
    }

    fn add_marker_slots(&self, batch_id: &str, slots: &[String]) -> StoreResult<()> {
        self.with(|t| {
            let marker = t
                .markers
                .get_mut(batch_id)
                .ok_or(StoreError::ConditionFailed)?;
            for slot in slots {
                if !marker.slots.contains(slot) {
                    marker.slots.push(slot.clone());
                }
            }
            Ok(())
        })
    }

    fn append_log(&self, item: &LogItem) -> StoreResult<()> {
        self.with(|t| {
            let key = (item.engine, item.hour, item.sort.clone());
            if t.log.contains_key(&key) {
                return Err(StoreError::ConditionFailed);
            }
            t.log.insert(key, item.clone());
            Ok(())
        })
    }

    fn query_log(&self, engine: u32, hour: u32) -> StoreResult<Vec<LogItem>> {
        self.with(|t| {
            Ok(t.log
                .range((engine, hour, String::new())..)
                .take_while(|((e, h, _), _)| *e == engine && *h == hour)
                .map(|(_, item)| item.clone())
                .collect())
        })
    }

    fn put_verify(&self, item: &VerifyItem) -> StoreResult<()> {
        self.with(|t| {
            let key = (item.engine, item.sort.clone());
            if t.verify.contains_key(&key) {
                return Err(StoreError::ConditionFailed);
            }
            t.verify.insert(key, item.clone());
            Ok(())
        })
    }

    fn get_verify(&self, engine: u32, sort: &str) -> StoreResult<Option<VerifyItem>> {
        self.with(|t| Ok(t.verify.get(&(engine, sort.to_string())).cloned()))
    }

    fn query_verify(
        &self,
        engine: u32,
        after: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<VerifyItem>> {
        self.with(|t| {
            Ok(t.verify
                .range((engine, String::new())..)
                .take_while(|((e, _), _)| *e == engine)
                .filter(|((_, sort), _)| after.is_none_or(|after| sort.as_str() > after))
                .take(limit)
                .map(|(_, item)| item.clone())
                .collect())
        })
    }

    fn claim_verify(
        &self,
        engine: u32,
        sort: &str,
        uid: &str,
        until: u64,
        now: u64,
    ) -> StoreResult<()> {
        self.with(|t| {
            let item = t
                .verify
                .get_mut(&(engine, sort.to_string()))
                .ok_or(StoreError::ConditionFailed)?;
            if item.uids.iter().any(|u| u == uid) || !item.claimable(now) {
                return Err(StoreError::ConditionFailed);
            }
            item.claimant = Some(uid.to_string());
            item.claim_until = Some(until);
            Ok(())
        })
    }

    fn replace_verify(&self, item: &VerifyItem, claimant: &str) -> StoreResult<()> {
        self.with(|t| {
            let existing = t
                .verify
                .get_mut(&(item.engine, item.sort.clone()))
                .ok_or(StoreError::ConditionFailed)?;
            if existing.claimant.as_deref() != Some(claimant) {
                return Err(StoreError::ConditionFailed);
            }
            *existing = item.clone();
            Ok(())
        })
    }

    fn delete_verify(&self, engine: u32, sort: &str, claimant: Option<&str>) -> StoreResult<()> {
        self.with(|t| {
            let key = (engine, sort.to_string());
            let existing = t.verify.get(&key).ok_or(StoreError::ConditionFailed)?;
            if claimant.is_some_and(|c| existing.claimant.as_deref() != Some(c)) {
                return Err(StoreError::ConditionFailed);
            }
            t.verify.remove(&key);
            Ok(())
        })
    }

    fn get_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
    ) -> StoreResult<Option<SlotItem>> {
        self.with(|t| Ok(t.slots.get(&(engine, division, key.to_string())).cloned()))
    }

    fn update_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
        expected: &[(String, Option<SlotEntry>)],
        set: &[(String, SlotEntry)],
        full: bool,
    ) -> StoreResult<()> {
        self.with(|t| {
            let item = t
                .slots
                .entry((engine, division, key.to_string()))
                .or_default();
            if expected
                .iter()
                .any(|(slot, entry)| item.entries.get(slot) != entry.as_ref())
            {
                return Err(StoreError::ConditionFailed);
            }
            for (slot, entry) in set {
                item.entries.insert(slot.clone(), entry.clone());
            }
            item.full = full;
            Ok(())
        })
    }

    fn put_alert(&self, alert: &Alert) -> StoreResult<()> {
        self.with(|t| {
            t.alerts.insert(alert.sort.clone(), alert.clone());
            Ok(())
        })
    }

    fn list_alerts(&self) -> StoreResult<Vec<Alert>> {
        self.with(|t| Ok(t.alerts.values().cloned().collect()))
    }

    fn delete_slots(&self, engine: u32, division: Division, key: &str) -> StoreResult<()> {
        self.with(|t| {
            t.slots.remove(&(engine, division, key.to_string()));
            Ok(())
        })
    }

    fn remove_slots(
        &self,
        engine: u32,
        division: Division,
        key: &str,
        remove: &[(String, SlotEntry)],
    ) -> StoreResult<()> {
        self.with(|t| {
            let Some(item) = t.slots.get_mut(&(engine, division, key.to_string())) else {
                return Err(StoreError::ConditionFailed);
            };
            if remove
                .iter()
                .any(|(slot, entry)| item.entries.get(slot) != Some(entry))
            {
                return Err(StoreError::ConditionFailed);
            }
            for (slot, _) in remove {
                item.entries.remove(slot);
            }
            item.full = false;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_793_890_800), "2026-11-05T15:00:00Z");
        assert_eq!(hour_key(1_793_890_800), 2_026_110_515);
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(hour_key(951_782_400 + 3_599), 2_000_022_900);
    }

    fn user(uid: &str) -> User {
        User {
            uid: uid.to_string(),
            ..User::default()
        }
    }

    #[test]
    fn user_conditions() {
        let store = MemStore::new();
        store.create_user(&user("g1")).unwrap();
        assert_eq!(
            store.create_user(&user("g1")),
            Err(StoreError::ConditionFailed)
        );
        let mut rep = Reputation {
            rep: 5,
            ..Reputation::default()
        };
        store.set_reputation("g1", 0, &rep).unwrap();
        assert_eq!(
            store.set_reputation("g1", 0, &rep),
            Err(StoreError::ConditionFailed)
        );
        rep.rep = 6;
        store.set_reputation("g1", 1, &rep).unwrap();
        let stored = store.get_user("g1").unwrap().unwrap();
        assert_eq!((stored.reputation.rep, stored.reputation.ver), (6, 2));
        assert_eq!(
            store.patch_user("g2", &UserPatch::default()),
            Err(StoreError::ConditionFailed)
        );
    }

    fn marker(id: &str) -> BatchMarker {
        BatchMarker {
            batch_id: id.to_string(),
            uid: "g1".to_string(),
            accepted: vec![0],
            rejected: vec![],
            slots: vec![],
            ttl: 0,
        }
    }

    #[test]
    fn results_transaction_is_all_or_nothing() {
        let store = MemStore::new();
        let log = LogItem {
            engine: 1,
            hour: 2_026_102_412,
            sort: "1#b".to_string(),
            uid: "g1".to_string(),
            records: vec![],
            seeds: vec![],
            battles: 1,
            held: false,
            ttl: 0,
        };
        let tx = ResultsTransaction {
            marker: marker("b"),
            log: Some(log),
            uid: "g1".to_string(),
            battles: 3,
            last_post: 9,
        };
        // No user: nothing is written.
        assert_eq!(store.commit_results(&tx), Err(StoreError::ConditionFailed));
        assert!(store.get_marker("b").unwrap().is_none());
        assert!(store.log_items(1).is_empty());
        store.create_user(&user("g1")).unwrap();
        store.commit_results(&tx).unwrap();
        assert_eq!(store.get_user("g1").unwrap().unwrap().battles, 3);
        // A second post of the batch changes nothing.
        assert_eq!(store.commit_results(&tx), Err(StoreError::ConditionFailed));
        assert_eq!(store.get_user("g1").unwrap().unwrap().battles, 3);
        assert_eq!(store.log_items(1).len(), 1);
        assert_eq!(store.query_log(1, 2_026_102_412).unwrap().len(), 1);
        assert!(store.query_log(1, 2_026_102_413).unwrap().is_empty());
    }

    #[test]
    fn verify_claims() {
        let store = MemStore::new();
        let item = VerifyItem {
            engine: 1,
            sort: "0001#a".to_string(),
            division: Division::OneVsOne,
            seed: 1,
            bots: vec![1, 2],
            rounds: 35,
            runs: vec![],
            uids: vec!["g1".to_string()],
            stage: 1,
            claimant: None,
            claim_until: None,
            kind: VerifyKind::Check,
            ttl: 0,
        };
        store.put_verify(&item).unwrap();
        assert_eq!(store.put_verify(&item), Err(StoreError::ConditionFailed));
        // Its own runner can't claim it.
        assert_eq!(
            store.claim_verify(1, "0001#a", "g1", 100, 0),
            Err(StoreError::ConditionFailed)
        );
        store.claim_verify(1, "0001#a", "g2", 100, 0).unwrap();
        assert_eq!(
            store.claim_verify(1, "0001#a", "g3", 200, 50),
            Err(StoreError::ConditionFailed)
        );
        // Expired claims can be taken over.
        store.claim_verify(1, "0001#a", "g3", 300, 100).unwrap();
        assert_eq!(
            store.delete_verify(1, "0001#a", Some("g2")),
            Err(StoreError::ConditionFailed)
        );
        store.delete_verify(1, "0001#a", Some("g3")).unwrap();
        assert!(store.get_verify(1, "0001#a").unwrap().is_none());
    }
}
