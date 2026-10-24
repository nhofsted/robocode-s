// SPDX-License-Identifier: EPL-2.0

//! Bots and versions (spec §12.2-§12.4, §16): registering a submitted or imported version
//! from a validated jar, retiring, claiming and taking down bots, the owner's bot list and
//! the admin's user update. The Lambda does the S3 side (fetching, copying the jar).

use crate::api::{
    ApiError, BotInfo, BotVersionInfo, ErrorCode, ImportRequest, ImportResponse, ImportResult,
    SubmitResponse, UserUpdateRequest,
};
use crate::auth::{Submission, check_submission, is_admin};
use crate::config::Config;
use crate::ids::Division;
use crate::jar::JarInfo;
use crate::store::{
    ALERT_TTL, Alert, BotItem, BotSource, BotState, DAY, Store, StoreError, StoreResult, User,
    UserPatch, UserState, VersionItem, VersionState, rfc3339,
};
use crate::verify::{ban_alert, update_reputation};

fn not_found(what: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::NotFound, what)
}

fn forbidden(what: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Forbidden, what)
}

fn conflict(what: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Conflict, what)
}

/// The version items of a bot, newest first (absent ones skipped).
pub fn bot_versions<S: Store + ?Sized>(store: &S, bot: &BotItem) -> StoreResult<Vec<VersionItem>> {
    let mut versions = Vec::new();
    for &vid in &bot.versions {
        if let Some(version) = store.get_version(vid)? {
            versions.push(version);
        }
    }
    Ok(versions)
}

fn sorted_divisions(divisions: impl IntoIterator<Item = Division>) -> Vec<Division> {
    let mut list: Vec<Division> = divisions.into_iter().collect();
    list.sort_unstable();
    list.dedup();
    list
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A `bots/submit` (§12.2) of the validated jar `info` (already in `jars/<sha256>.jar`) by
/// `user`: the limits and ownership of [`check_submission`], then a new version on
/// probation in one transaction with the bot item (created for a new bot, which then
/// belongs to `user`). The same version with the same jar answers again without a change;
/// with another jar it is 409.
#[allow(clippy::too_many_arguments)]
pub fn submit<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    user: &User,
    info: &JarInfo,
    divisions: &[Division],
    homepage: Option<&str>,
    source_url: Option<&str>,
    now: u64,
) -> Result<SubmitResponse, ApiError> {
    if divisions.is_empty() {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            "divisions: at least one",
        ));
    }
    let user = store
        .get_user(&user.uid)?
        .ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "unknown user"))?;
    let bot = store.get_bot(&info.bot_id)?;
    if bot.as_ref().is_some_and(|b| b.state == BotState::Removed) {
        return Err(forbidden(format!("{} was taken down", info.bot_id)));
    }
    let versions = match &bot {
        Some(bot) => bot_versions(store, bot)?,
        None => Vec::new(),
    };
    let response = |version: &VersionItem| SubmitResponse {
        bot: version.bot.clone(),
        version: version.version.clone(),
        vid: version.vid,
        state: version.state.as_str().to_string(),
        codesize: version.codesize.unwrap_or(info.codesize),
        weight_class: version.class,
    };
    let same = versions.iter().find(|v| v.version == info.version);
    if let (Some(existing), Some(bot)) = (same, &bot)
        && existing.sha256 == info.sha256
        && bot.owner.as_deref() == Some(user.uid.as_str())
    {
        return Ok(response(existing));
    }
    let last_day = versions.iter().filter(|v| v.created + DAY > now).count() as u32;
    check_submission(
        config,
        &user,
        &Submission {
            bot: bot.as_ref(),
            owned_bots: user.bots.len() as u32,
            versions_last_day: last_day,
            import: false,
        },
        now,
    )?;
    if let Some(existing) = same {
        return if existing.sha256 == info.sha256 {
            Ok(response(existing))
        } else {
            Err(conflict(format!(
                "{} {} is registered with another jar",
                info.bot_id, info.version
            )))
        };
    }

    let vid = store.next_vid()?;
    let version = VersionItem {
        vid,
        bot: info.bot_id.clone(),
        version: info.version.clone(),
        sha256: info.sha256.clone(),
        size: info.size,
        codesize: Some(info.codesize),
        class: info.weight_class,
        divisions: sorted_divisions(divisions.iter().copied()),
        state: VersionState::Probation,
        probation_battles: 0,
        probation_failures: 0,
        class_name: info.main_class.clone(),
        created: now,
        retired: None,
        url: source_url.map(str::to_string),
    };
    let homepage = homepage.and_then(non_empty);
    let item = match &bot {
        None => BotItem {
            id: info.bot_id.clone(),
            author: non_empty(&info.author).unwrap_or_else(|| user.login.clone()),
            owner: Some(user.uid.clone()),
            homepage: homepage.or_else(|| non_empty(&info.webpage)),
            description: non_empty(&info.description),
            divisions: version.divisions.clone(),
            versions: vec![vid],
            current: None,
            state: BotState::Active,
            created: now,
            source: BotSource::Submit,
        },
        Some(bot) => {
            let mut bot = bot.clone();
            bot.versions.insert(0, vid);
            bot.divisions = sorted_divisions(bot.divisions.iter().chain(divisions).copied());
            if homepage.is_some() {
                bot.homepage = homepage;
            }
            if let Some(description) = non_empty(&info.description) {
                bot.description = Some(description);
            }
            bot.state = BotState::Active;
            bot
        }
    };
    let expected = bot.as_ref().map(|b| b.versions.as_slice());
    match store.register_version(&item, expected, &version) {
        Ok(()) => {}
        Err(StoreError::ConditionFailed) => {
            return Err(conflict("a concurrent submission of this bot; retry"));
        }
        Err(e) => return Err(e.into()),
    }
    if bot.is_none() {
        add_owned(store, &user.uid, &item.id)?;
    }
    Ok(response(&version))
}

fn add_owned<S: Store + ?Sized>(store: &S, uid: &str, bot: &str) -> StoreResult<()> {
    let Some(user) = store.get_user(uid)? else {
        return Ok(());
    };
    if user.bots.iter().any(|b| b == bot) {
        return Ok(());
    }
    let mut bots = user.bots;
    bots.push(bot.to_string());
    store.patch_user(
        uid,
        &UserPatch {
            bots: Some(bots),
            ..UserPatch::default()
        },
    )
}

fn remove_owned<S: Store + ?Sized>(store: &S, uid: &str, bot: &str) -> StoreResult<()> {
    let Some(user) = store.get_user(uid)? else {
        return Ok(());
    };
    if !user.bots.iter().any(|b| b == bot) {
        return Ok(());
    }
    let bots = user.bots.into_iter().filter(|b| b != bot).collect();
    store.patch_user(
        uid,
        &UserPatch {
            bots: Some(bots),
            ..UserPatch::default()
        },
    )
}

/// Validates an imported jar (§16.2): with `versionFromList`, a jar without `robot.version`
/// takes the entry's version (`jar::validate_with_version`); otherwise plain
/// `jar::validate`. Failures are 400 with the messages.
pub fn validate_import(
    bytes: &[u8],
    config: &Config,
    request: &ImportRequest,
) -> Result<JarInfo, ApiError> {
    let result = if request.version_from_list {
        crate::jar::validate_with_version(bytes, config, &request.version).map(|(info, _)| info)
    } else {
        crate::jar::validate(bytes, config)
    };
    result.map_err(|messages| {
        ApiError::new(
            ErrorCode::BadRequest,
            format!("invalid ({})", messages.join("; ")),
        )
    })
}

/// `admin/bots/import` (§16.2) of the validated jar `info`: the jar's properties must match
/// the entry. A new version is registered **active** (no probation) and the bot's previous
/// current version retired; an existing version with the same jar is `exists`, or
/// `divisionAdded` when the entry's division is new for it; with another jar 409.
pub fn import<S: Store + ?Sized>(
    store: &S,
    request: &ImportRequest,
    info: &JarInfo,
    now: u64,
) -> Result<ImportResponse, ApiError> {
    if info.bot_id != request.bot || info.version != request.version {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("mismatch (properties say {} {})", info.bot_id, info.version),
        ));
    }
    let division = request.division;
    let bot = store.get_bot(&info.bot_id)?;
    let versions = match &bot {
        Some(bot) => bot_versions(store, bot)?,
        None => Vec::new(),
    };
    if let Some(existing) = versions.iter().find(|v| v.version == info.version) {
        if existing.sha256 != info.sha256 {
            return Err(conflict(format!(
                "{} {} is registered with another jar",
                info.bot_id, info.version
            )));
        }
        let bot = bot.expect("versions come from the bot");
        if existing.divisions.contains(&division) && bot.divisions.contains(&division) {
            return Ok(ImportResponse {
                vid: existing.vid,
                result: ImportResult::Exists,
            });
        }
        let mut version = existing.clone();
        version.divisions = sorted_divisions(version.divisions.iter().copied().chain([division]));
        store.put_version(&version)?;
        let mut bot = bot;
        bot.divisions = sorted_divisions(bot.divisions.iter().copied().chain([division]));
        store.put_bot(&bot)?;
        return Ok(ImportResponse {
            vid: existing.vid,
            result: ImportResult::DivisionAdded,
        });
    }

    let vid = store.next_vid()?;
    let divisions = sorted_divisions(
        bot.iter()
            .flat_map(|b| b.divisions.iter().copied())
            .chain([division]),
    );
    let version = VersionItem {
        vid,
        bot: info.bot_id.clone(),
        version: info.version.clone(),
        sha256: info.sha256.clone(),
        size: info.size,
        codesize: Some(info.codesize),
        class: info.weight_class,
        divisions: divisions.clone(),
        state: VersionState::Active,
        probation_battles: 0,
        probation_failures: 0,
        class_name: info.main_class.clone(),
        created: now,
        retired: None,
        url: request.source_url.clone(),
    };
    let author = request
        .author
        .as_deref()
        .and_then(non_empty)
        .or_else(|| non_empty(&info.author))
        .unwrap_or_else(|| {
            info.bot_id
                .split('.')
                .next()
                .unwrap_or_default()
                .to_string()
        });
    let previous = bot.as_ref().and_then(|b| b.current);
    let item = match &bot {
        None => BotItem {
            id: info.bot_id.clone(),
            author,
            owner: None,
            homepage: non_empty(&info.webpage),
            description: non_empty(&info.description),
            divisions,
            versions: vec![vid],
            current: Some(vid),
            state: BotState::Active,
            created: now,
            source: BotSource::Import,
        },
        Some(bot) => {
            let mut bot = bot.clone();
            bot.versions.insert(0, vid);
            bot.divisions = divisions;
            bot.current = Some(vid);
            if bot.state == BotState::Retired {
                bot.state = BotState::Active;
            }
            bot
        }
    };
    let expected = bot.as_ref().map(|b| b.versions.as_slice());
    match store.register_version(&item, expected, &version) {
        Ok(()) => {}
        Err(StoreError::ConditionFailed) => {
            return Err(conflict("a concurrent registration of this bot; retry"));
        }
        Err(e) => return Err(e.into()),
    }
    if let Some(old) = previous.filter(|&old| old != vid)
        && let Some(mut old) = store.get_version(old)?
        && old.state == VersionState::Active
    {
        old.state = VersionState::Retired;
        old.retired = Some(now);
        store.put_version(&old)?;
    }
    Ok(ImportResponse {
        vid,
        result: ImportResult::Created,
    })
}

/// `bots/retire` (§12.3): the owner (or an admin) retires the bot: the bot, its current
/// version and any version on probation become `retired`.
pub fn retire<S: Store + ?Sized>(
    store: &S,
    config: &Config,
    user: &User,
    bot_id: &str,
    now: u64,
) -> Result<BotItem, ApiError> {
    let mut bot = store
        .get_bot(bot_id)?
        .ok_or_else(|| not_found(format!("no bot {bot_id}")))?;
    if bot.owner.as_deref() != Some(user.uid.as_str()) && !is_admin(config, user) {
        return Err(forbidden(format!("{bot_id} is not yours")));
    }
    if bot.state == BotState::Removed {
        return Err(forbidden(format!("{bot_id} was taken down")));
    }
    for mut version in bot_versions(store, &bot)? {
        if matches!(
            version.state,
            VersionState::Active | VersionState::Probation
        ) {
            version.state = VersionState::Retired;
            version.retired = Some(now);
            store.put_version(&version)?;
        }
    }
    bot.state = BotState::Retired;
    store.put_bot(&bot)?;
    Ok(bot)
}

/// `admin/bots/claim` (§16.4): gives the bot to the user `uid`, who must have signed in.
pub fn claim<S: Store + ?Sized>(store: &S, bot_id: &str, uid: &str) -> Result<BotItem, ApiError> {
    let mut bot = store
        .get_bot(bot_id)?
        .ok_or_else(|| not_found(format!("no bot {bot_id}")))?;
    if store.get_user(uid)?.is_none() {
        return Err(not_found(format!(
            "no user {uid}: they must sign in once first"
        )));
    }
    if let Some(old) = bot.owner.as_deref().filter(|&old| old != uid) {
        remove_owned(store, old, bot_id)?;
    }
    bot.owner = Some(uid.to_string());
    store.put_bot(&bot)?;
    add_owned(store, uid, bot_id)?;
    Ok(bot)
}

/// What a takedown did: the bot, the line to log (`ALERT takedown ...`), and the removed
/// versions' jar hashes (their jars and recordings are deleted by the `cleanup` job).
#[derive(Clone, Debug, PartialEq)]
pub struct Removed {
    pub bot: BotItem,
    pub log: String,
    pub jars: Vec<String>,
}

/// `admin/bots/remove` (§12.3, §12.4): the bot and every version become `removed`, the
/// owner loses it, and an alert records the takedown. `now_ms` in milliseconds.
pub fn remove<S: Store + ?Sized>(
    store: &S,
    bot_id: &str,
    reason: &str,
    now_ms: u64,
) -> Result<Removed, ApiError> {
    let mut bot = store
        .get_bot(bot_id)?
        .ok_or_else(|| not_found(format!("no bot {bot_id}")))?;
    let now = now_ms / 1000;
    let mut jars = Vec::new();
    for mut version in bot_versions(store, &bot)? {
        jars.push(version.sha256.clone());
        if version.state != VersionState::Removed {
            version.state = VersionState::Removed;
            version.retired.get_or_insert(now);
            store.put_version(&version)?;
        }
    }
    bot.state = BotState::Removed;
    store.put_bot(&bot)?;
    if let Some(owner) = &bot.owner {
        remove_owned(store, owner, bot_id)?;
    }
    let reason: String = reason
        .chars()
        .filter(|c| !c.is_control())
        .take(500)
        .collect();
    store.put_alert(&Alert {
        sort: format!("{now_ms:013}#takedown.{bot_id}"),
        kind: "takedown".to_string(),
        text: format!("{bot_id} removed: {reason}"),
        reference: bot_id.to_string(),
        ttl: now + ALERT_TTL,
    })?;
    Ok(Removed {
        log: format!("ALERT takedown {bot_id} {reason}"),
        bot,
        jars,
    })
}

/// `GET me/bots`: the user's bots with their versions, newest first.
pub fn list_bots<S: Store + ?Sized>(store: &S, user: &User) -> StoreResult<Vec<BotInfo>> {
    let mut bots = Vec::new();
    for id in &user.bots {
        let Some(bot) = store.get_bot(id)? else {
            continue;
        };
        let versions = bot_versions(store, &bot)?
            .into_iter()
            .map(|v| BotVersionInfo {
                vid: v.vid,
                version: v.version,
                state: v.state.as_str().to_string(),
                sha256: v.sha256,
                size: v.size,
                codesize: v.codesize,
                weight_class: v.class,
                divisions: v.divisions,
                created: rfc3339(v.created),
                probation_battles: v.probation_battles,
                probation_failures: v.probation_failures,
            })
            .collect();
        bots.push(BotInfo {
            id: bot.id,
            state: bot.state.as_str().to_string(),
            author: bot.author,
            homepage: bot.homepage,
            divisions: bot.divisions,
            current: bot.current,
            versions,
        });
    }
    Ok(bots)
}

/// `admin/users/update` (§11.1): sets reputation and state (a return to `new` clears the
/// quarantine count), the admin flag and the account-age exemption (`tr`). A ban leaves the
/// alert the `cleanup` job looks for (`verify::ban_alert`). Returns the user. `now_ms` in
/// milliseconds.
pub fn update_user<S: Store + ?Sized>(
    store: &S,
    request: &UserUpdateRequest,
    now_ms: u64,
) -> Result<User, ApiError> {
    let state = request
        .state
        .as_deref()
        .map(UserState::parse)
        .transpose()
        .map_err(|e| ApiError::new(ErrorCode::BadRequest, e))?;
    let Some(before) = store.get_user(&request.uid)? else {
        return Err(not_found(format!("no user {}", request.uid)));
    };
    if state == Some(UserState::Banned) && before.reputation.state != UserState::Banned {
        store.put_alert(&ban_alert(&request.uid, "banned by an admin", now_ms))?;
    }
    if request.rep.is_some() || state.is_some() {
        update_reputation(store, &request.uid, |reputation| {
            if let Some(rep) = request.rep {
                reputation.rep = rep;
            }
            if let Some(state) = state {
                if state != reputation.state {
                    reputation.quarantine_matches = 0;
                }
                reputation.state = state;
            }
        })?;
    }
    let patch = UserPatch {
        admin: request.admin,
        trusted: request.trusted,
        ..UserPatch::default()
    };
    if patch != UserPatch::default() {
        store.patch_user(&request.uid, &patch)?;
    }
    store
        .get_user(&request.uid)?
        .ok_or_else(|| not_found(format!("no user {}", request.uid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WeightClass;
    use crate::store::{MemStore, Reputation};

    const NOW: u64 = 1_792_843_200;

    fn info(bot: &str, version: &str, sha: char) -> JarInfo {
        JarInfo {
            bot_id: bot.into(),
            version: version.into(),
            main_class: bot.into(),
            author: "Ann".into(),
            description: "a bot".into(),
            webpage: "https://example.org/bot".into(),
            codesize: 600,
            weight_class: WeightClass::Micro,
            sha256: sha.to_string().repeat(64),
            size: 1234,
        }
    }

    fn store_with(uid: &str, github_created: u64) -> MemStore {
        let store = MemStore::new();
        store
            .create_user(&User {
                uid: uid.into(),
                login: uid.into(),
                github_id: 1,
                github_created: Some(github_created),
                ..User::default()
            })
            .unwrap();
        store
    }

    fn user(store: &MemStore, uid: &str) -> User {
        store.get_user(uid).unwrap().unwrap()
    }

    #[test]
    fn submissions() {
        let config = Config::default();
        let store = store_with("g1", NOW - 400 * DAY);
        let one = [Division::OneVsOne];
        let first = submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("a.B", "1", 'a'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap();
        assert_eq!(
            (first.vid, first.state.as_str(), first.weight_class),
            (1, "probation", WeightClass::Micro)
        );
        let bot = store.get_bot("a.B").unwrap().unwrap();
        assert_eq!(bot.owner.as_deref(), Some("g1"));
        assert_eq!(bot.source, BotSource::Submit);
        assert_eq!(bot.homepage.as_deref(), Some("https://example.org/bot"));
        assert_eq!(bot.current, None, "ranked only after probation");
        assert_eq!(user(&store, "g1").bots, vec!["a.B".to_string()]);

        // The same version and jar again: nothing changes.
        let again = submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("a.B", "1", 'a'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap();
        assert_eq!(again, first);
        // Another jar for that version: 409.
        let err = submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("a.B", "1", 'b'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        // A new version is prepended.
        let second = submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("a.B", "2", 'b'),
            &[Division::Melee],
            Some("https://new.example"),
            Some("https://x/y.jar"),
            NOW,
        )
        .unwrap();
        assert_eq!(second.vid, 2);
        let bot = store.get_bot("a.B").unwrap().unwrap();
        assert_eq!(bot.versions, vec![2, 1]);
        assert_eq!(bot.divisions, vec![Division::OneVsOne, Division::Melee]);
        assert_eq!(bot.homepage.as_deref(), Some("https://new.example"));
        assert_eq!(
            store.get_version(2).unwrap().unwrap().url.as_deref(),
            Some("https://x/y.jar")
        );

        // Someone else can't add versions.
        store
            .create_user(&User {
                uid: "g2".into(),
                github_created: Some(NOW - 400 * DAY),
                ..User::default()
            })
            .unwrap();
        let err = submit(
            &store,
            &config,
            &user(&store, "g2"),
            &info("a.B", "3", 'c'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
        assert!(
            submit(
                &store,
                &config,
                &user(&store, "g2"),
                &info("a.B", "1", 'a'),
                &one,
                None,
                None,
                NOW
            )
            .is_err()
        );

        // Versions per day.
        for v in 3..=5 {
            submit(
                &store,
                &config,
                &user(&store, "g1"),
                &info("a.B", &v.to_string(), 'c'),
                &one,
                None,
                None,
                NOW,
            )
            .unwrap();
        }
        let err = submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("a.B", "6", 'd'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap_err();
        assert!(err.message.contains("per day"), "{err}");
        assert!(
            submit(
                &store,
                &config,
                &user(&store, "g1"),
                &info("a.B", "6", 'd'),
                &one,
                None,
                None,
                NOW + DAY
            )
            .is_ok()
        );

        // Bots per user.
        for b in 2..=5 {
            submit(
                &store,
                &config,
                &user(&store, "g1"),
                &info(&format!("a.B{b}"), "1", 'e'),
                &one,
                None,
                None,
                NOW,
            )
            .unwrap();
        }
        let err = submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("a.B6", "1", 'e'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap_err();
        assert!(err.message.contains("bots per user"), "{err}");
        assert!(
            submit(
                &store,
                &config,
                &user(&store, "g1"),
                &info("a.B6", "1", 'e'),
                &[],
                None,
                None,
                NOW
            )
            .is_err()
        );

        // Young accounts.
        let young = store_with("g3", NOW - 10 * DAY);
        let err = submit(
            &young,
            &config,
            &user(&young, "g3"),
            &info("y.Z", "1", 'a'),
            &one,
            None,
            None,
            NOW,
        )
        .unwrap_err();
        assert!(err.message.contains("approval"), "{err}");
    }

    fn import_request(bot: &str, version: &str, sha: char, division: Division) -> ImportRequest {
        ImportRequest {
            bot: bot.into(),
            version: version.into(),
            sha256: sha.to_string().repeat(64),
            division,
            author: Some("Julian".into()),
            source_url: Some("http://x/y.jar".into()),
            source: Some("list".into()),
            version_from_list: false,
        }
    }

    #[test]
    fn imports() {
        let store = MemStore::new();
        let request = import_request("jk.B", "1", 'a', Division::OneVsOne);
        let created = import(&store, &request, &info("jk.B", "1", 'a'), NOW).unwrap();
        assert_eq!(
            created,
            ImportResponse {
                vid: 1,
                result: ImportResult::Created
            }
        );
        let bot = store.get_bot("jk.B").unwrap().unwrap();
        assert_eq!(
            (bot.current, bot.owner.clone(), bot.source),
            (Some(1), None, BotSource::Import)
        );
        assert_eq!(bot.author, "Julian");
        assert_eq!(
            store.get_version(1).unwrap().unwrap().state,
            VersionState::Active
        );

        assert_eq!(
            import(&store, &request, &info("jk.B", "1", 'a'), NOW)
                .unwrap()
                .result,
            ImportResult::Exists
        );
        let melee = import_request("jk.B", "1", 'a', Division::Melee);
        assert_eq!(
            import(&store, &melee, &info("jk.B", "1", 'a'), NOW)
                .unwrap()
                .result,
            ImportResult::DivisionAdded
        );
        assert_eq!(
            store.get_bot("jk.B").unwrap().unwrap().divisions,
            Division::ALL.to_vec()
        );
        assert_eq!(
            store.get_version(1).unwrap().unwrap().divisions,
            Division::ALL.to_vec()
        );
        assert_eq!(
            import(&store, &melee, &info("jk.B", "1", 'a'), NOW)
                .unwrap()
                .result,
            ImportResult::Exists
        );

        let err = import(
            &store,
            &import_request("jk.B", "1", 'b', Division::OneVsOne),
            &info("jk.B", "1", 'b'),
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        let err = import(
            &store,
            &import_request("jk.B", "2", 'b', Division::OneVsOne),
            &info("jk.B", "3", 'b'),
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.message.contains("properties say jk.B 3"));

        // A newer version replaces the current one, keeping the divisions.
        let newer = import(
            &store,
            &import_request("jk.B", "2", 'b', Division::OneVsOne),
            &info("jk.B", "2", 'b'),
            NOW + 5,
        )
        .unwrap();
        assert_eq!(newer.vid, 2);
        let bot = store.get_bot("jk.B").unwrap().unwrap();
        assert_eq!((bot.current, bot.versions.clone()), (Some(2), vec![2, 1]));
        let old = store.get_version(1).unwrap().unwrap();
        assert_eq!(
            (old.state, old.retired),
            (VersionState::Retired, Some(NOW + 5))
        );
        assert_eq!(
            store.get_version(2).unwrap().unwrap().divisions,
            Division::ALL.to_vec()
        );
    }

    #[test]
    fn retire_claim_remove_and_list() {
        let config = Config::default();
        let store = store_with("g1", NOW - 400 * DAY);
        store
            .create_user(&User {
                uid: "g2".into(),
                github_id: 2,
                ..User::default()
            })
            .unwrap();
        import(
            &store,
            &import_request("jk.B", "1", 'a', Division::OneVsOne),
            &info("jk.B", "1", 'a'),
            NOW,
        )
        .unwrap();
        assert_eq!(
            claim(&store, "jk.B", "g9").unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            claim(&store, "no.Such", "g1").unwrap_err().code,
            ErrorCode::NotFound
        );
        claim(&store, "jk.B", "g1").unwrap();
        assert_eq!(user(&store, "g1").bots, vec!["jk.B".to_string()]);
        // Now the owner can submit a version.
        submit(
            &store,
            &config,
            &user(&store, "g1"),
            &info("jk.B", "2", 'b'),
            &[Division::OneVsOne],
            None,
            None,
            NOW,
        )
        .unwrap();
        let listed = list_bots(&store, &user(&store, "g1")).unwrap();
        assert_eq!(listed.len(), 1);
        let states: Vec<&str> = listed[0]
            .versions
            .iter()
            .map(|v| v.state.as_str())
            .collect();
        assert_eq!(states, ["probation", "active"]);
        assert_eq!(listed[0].current, Some(1));

        // Reassigning moves it between the users' lists.
        claim(&store, "jk.B", "g2").unwrap();
        assert!(user(&store, "g1").bots.is_empty());
        assert_eq!(user(&store, "g2").bots, vec!["jk.B".to_string()]);
        assert_eq!(
            retire(&store, &config, &user(&store, "g1"), "jk.B", NOW)
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        let retired = retire(&store, &config, &user(&store, "g2"), "jk.B", NOW + 9).unwrap();
        assert_eq!(retired.state, BotState::Retired);
        for vid in [1, 2] {
            let v = store.get_version(vid).unwrap().unwrap();
            assert_eq!((v.state, v.retired), (VersionState::Retired, Some(NOW + 9)));
        }

        let removed = remove(&store, "jk.B", "copyright\n", NOW * 1000).unwrap();
        assert_eq!(removed.bot.state, BotState::Removed);
        assert_eq!(removed.log, "ALERT takedown jk.B copyright");
        assert_eq!(removed.jars, vec!["b".repeat(64), "a".repeat(64)]);
        assert!(user(&store, "g2").bots.is_empty());
        assert_eq!(
            store.get_version(1).unwrap().unwrap().state,
            VersionState::Removed
        );
        let alerts = store.list_alerts().unwrap();
        assert_eq!((alerts.len(), alerts[0].kind.as_str()), (1, "takedown"));
        assert_eq!(
            retire(&store, &config, &user(&store, "g2"), "jk.B", NOW)
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
    }

    #[test]
    fn user_updates() {
        let store = store_with("g1", NOW);
        store
            .set_reputation(
                "g1",
                0,
                &Reputation {
                    state: UserState::Quarantined,
                    quarantine_matches: 4,
                    ..Reputation::default()
                },
            )
            .unwrap();
        let updated = update_user(
            &store,
            &UserUpdateRequest {
                uid: "g1".into(),
                rep: Some(1000),
                state: Some("new".into()),
                admin: Some(true),
                trusted: Some(true),
            },
            NOW * 1000,
        )
        .unwrap();
        assert_eq!(updated.reputation.rep, 1000);
        assert_eq!(updated.reputation.state, UserState::New);
        assert_eq!(updated.reputation.quarantine_matches, 0);
        assert!(updated.admin && updated.trusted);
        let bad = UserUpdateRequest {
            uid: "g1".into(),
            state: Some("zombie".into()),
            ..UserUpdateRequest::default()
        };
        assert_eq!(
            update_user(&store, &bad, NOW * 1000).unwrap_err().code,
            ErrorCode::BadRequest
        );
        let missing = UserUpdateRequest {
            uid: "g7".into(),
            ..UserUpdateRequest::default()
        };
        assert_eq!(
            update_user(&store, &missing, NOW * 1000).unwrap_err().code,
            ErrorCode::NotFound
        );
    }
}
