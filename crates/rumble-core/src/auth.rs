// SPDX-License-Identifier: EPL-2.0

//! Authentication logic (spec §7) and the submission limits (§12.2): upload tokens and
//! sessions (generation from caller-supplied random bytes, SHA-256 hashes, lookup, renewal,
//! revocation), the CSRF origin check, the user upsert from a GitHub profile, the admin
//! check and the `me` view.

use crate::api::{ApiError, ErrorCode, Me};
use crate::config::Config;
use crate::store::{
    BotItem, DAY, Store, StoreError, StoreResult, TokenItem, TokenKind, User, UserPatch, UserState,
    VersionItem, rfc3339,
};
use crate::verify::{tier, verify_rate};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

pub const UPLOAD_PREFIX: &str = "rsu_";
pub const SESSION_PREFIX: &str = "rss_";
/// Session lifetime (§7.1).
pub const SESSION_SECONDS: u64 = 30 * DAY;
/// Sessions are renewed when used with less than this left.
pub const RENEW_BELOW: u64 = 7 * DAY;
/// `lu` is written at most this often.
pub const LAST_USED_EVERY: u64 = DAY;

/// A new token of the kind: its prefix and the base64url of 32 random bytes (43 characters).
pub fn new_token(kind: TokenKind, random: &[u8; 32]) -> String {
    let prefix = match kind {
        TokenKind::Upload => UPLOAD_PREFIX,
        TokenKind::Session => SESSION_PREFIX,
    };
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(random))
}

/// The kind a token claims by its prefix, if it has a valid shape.
pub fn token_kind(token: &str) -> Option<TokenKind> {
    let (kind, rest) = if let Some(rest) = token.strip_prefix(UPLOAD_PREFIX) {
        (TokenKind::Upload, rest)
    } else {
        (TokenKind::Session, token.strip_prefix(SESSION_PREFIX)?)
    };
    let ok = rest.len() == 43
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(kind)
}

/// The SHA-256 hex of a token: the only form stored (§7.3).
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Creates and stores a token or session for `uid`. Returns the token (shown once) and
/// its item. Sessions expire after [`SESSION_SECONDS`].
pub fn issue_token<S: Store + ?Sized>(
    store: &S,
    uid: &str,
    kind: TokenKind,
    label: &str,
    random: &[u8; 32],
    now: u64,
) -> StoreResult<(String, TokenItem)> {
    let token = new_token(kind, random);
    let item = TokenItem {
        hash: hash_token(&token),
        uid: uid.to_string(),
        kind,
        label: label.chars().take(100).collect(),
        created: now,
        last_used: now,
        expires: (kind == TokenKind::Session).then_some(now + SESSION_SECONDS),
    };
    store.put_token(&item)?;
    Ok((token, item))
}

/// Whether a session expiring at `expires` is renewed when used at `now` (§7.1).
pub fn session_needs_renewal(expires: u64, now: u64) -> bool {
    expires > now && expires - now < RENEW_BELOW
}

/// A successful authentication.
#[derive(Clone, Debug, PartialEq)]
pub struct Authenticated {
    pub user: User,
    pub token: TokenItem,
    /// The session's new expiry if it was renewed: the Lambda sets the cookie again.
    pub renewed: Option<u64>,
}

/// Authenticates a token or session (§7.3): unknown, malformed or expired → 401, a banned
/// user → 403. Renews a session in its last 7 days and updates `lu` at most daily (one write,
/// only when due). The Lambda caches the token and user lookups for 5 minutes.
pub fn authenticate<S: Store + ?Sized>(
    store: &S,
    token: &str,
    now: u64,
) -> Result<Authenticated, ApiError> {
    let unauthorized = || ApiError::new(ErrorCode::Unauthorized, "unknown or expired token");
    let kind = token_kind(token).ok_or_else(unauthorized)?;
    let mut item = store
        .get_token(&hash_token(token))?
        .ok_or_else(unauthorized)?;
    if item.kind != kind || item.expires.is_some_and(|expires| expires <= now) {
        return Err(unauthorized());
    }
    let user = store.get_user(&item.uid)?.ok_or_else(unauthorized)?;
    if user.reputation.state == UserState::Banned {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "this account is banned",
        ));
    }
    let renewed = item
        .expires
        .filter(|&expires| session_needs_renewal(expires, now))
        .map(|_| now + SESSION_SECONDS);
    if renewed.is_some() || now >= item.last_used + LAST_USED_EVERY {
        match store.touch_token(&item.hash, now, renewed) {
            Ok(()) | Err(StoreError::ConditionFailed) => {}
            Err(e) => return Err(e.into()),
        }
        item.last_used = now;
        if renewed.is_some() {
            item.expires = renewed;
        }
    }
    Ok(Authenticated {
        user,
        token: item,
        renewed,
    })
}

/// Revokes one of the user's tokens by its id (the first 12 hex digits of the hash) or
/// `"current"` (`current_hash`). Unknown ids are 404.
pub fn revoke<S: Store + ?Sized>(
    store: &S,
    uid: &str,
    id: &str,
    current_hash: &str,
) -> Result<(), ApiError> {
    let hash = if id == "current" {
        current_hash.to_string()
    } else {
        store
            .list_tokens(uid)?
            .into_iter()
            .find(|token| token.id() == id)
            .map(|token| token.hash)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such token"))?
    };
    store.delete_token(uid, &hash)?;
    Ok(())
}

/// The CSRF check for cookie-authenticated POSTs (§7.1): `Origin` must be the site's origin.
pub fn check_origin(origin: Option<&str>, site_origin: &str) -> Result<(), ApiError> {
    if origin == Some(site_origin) {
        Ok(())
    } else {
        Err(ApiError::new(ErrorCode::Forbidden, "bad request origin"))
    }
}

/// The GitHub profile fields the server keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct GithubProfile {
    pub id: u64,
    pub login: String,
    pub name: Option<String>,
    /// `created_at`, unix s.
    pub created_at: Option<u64>,
}

/// Creates the user on first sign-in, or updates the login, name and account creation time
/// when they changed (§7.1, §12.2 `ghc`). Returns the user as stored.
pub fn upsert_user<S: Store + ?Sized>(
    store: &S,
    profile: &GithubProfile,
    now: u64,
) -> StoreResult<User> {
    let uid = crate::ids::uid(profile.id);
    let name = profile.name.clone().unwrap_or_default();
    for _ in 0..2 {
        if let Some(mut user) = store.get_user(&uid)? {
            let patch = UserPatch {
                login: (user.login != profile.login).then(|| profile.login.clone()),
                name: (user.name != name).then(|| name.clone()),
                github_created: profile
                    .created_at
                    .filter(|&created| user.github_created != Some(created)),
                ..UserPatch::default()
            };
            if patch != UserPatch::default() {
                store.patch_user(&uid, &patch)?;
                patch.apply(&mut user);
            }
            return Ok(user);
        }
        let user = User {
            uid: uid.clone(),
            login: profile.login.clone(),
            name: name.clone(),
            github_id: profile.id,
            github_created: profile.created_at,
            created: now,
            ..User::default()
        };
        match store.create_user(&user) {
            Ok(()) => return Ok(user),
            // Created concurrently: update that one.
            Err(StoreError::ConditionFailed) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Busy)
}

/// Whether the user has the admin role: listed in `admins` or flagged `adm`.
pub fn is_admin(config: &Config, user: &User) -> bool {
    user.admin || config.admins.contains(&user.github_id)
}

/// The `GET me` body.
pub fn me(config: &Config, user: &User) -> Me {
    let reputation = &user.reputation;
    Me {
        uid: user.uid.clone(),
        login: user.login.clone(),
        name: user.name.clone(),
        country: user.country.clone(),
        reputation: i64::from(reputation.rep),
        tier: tier(reputation, config),
        verify_rate: verify_rate(reputation.rep, config),
        battles: user.battles,
        verified_matches: reputation.verified_matches,
        mismatches: reputation.mismatches,
        admin: is_admin(config, user),
        since: rfc3339(user.created),
    }
}

/// How many versions of `bot` were registered in the 24 hours before `now`.
pub fn versions_last_day(versions: &[VersionItem], bot: &str, now: u64) -> u32 {
    versions
        .iter()
        .filter(|v| v.bot == bot && v.created + DAY > now)
        .count() as u32
}

/// What a submission check needs to know besides the user.
#[derive(Clone, Copy, Debug)]
pub struct Submission<'a> {
    /// The bot item if the bot exists.
    pub bot: Option<&'a BotItem>,
    /// The submitter's owned bots (`User::bots`).
    pub owned_bots: u32,
    /// Versions of this bot registered in the last 24 hours ([`versions_last_day`]).
    pub versions_last_day: u32,
    /// An admin import (§16): exempt from every limit and from ownership.
    pub import: bool,
}

/// The submission rules of §12.2 (403 `forbidden` with a message): an existing bot only from
/// its owner (an unclaimed one needs a claim first), at most `submit.maxBots` bots per user,
/// `submit.versionsPerDay` new versions per bot per day, and a GitHub account at least
/// `submit.minAccountDays` old unless the owner trusted it (`tr`). Admins and imports are
/// exempt from the limits; admins still need ownership for `bots/submit`. An account whose
/// creation time is unknown counts as young.
pub fn check_submission(
    config: &Config,
    user: &User,
    submission: &Submission,
    now: u64,
) -> Result<(), ApiError> {
    let forbidden = |message: String| Err(ApiError::new(ErrorCode::Forbidden, message));
    if submission.import {
        return Ok(());
    }
    if user.reputation.state == UserState::Banned {
        return forbidden("this account is banned".to_string());
    }
    if let Some(bot) = submission.bot {
        match &bot.owner {
            Some(owner) if *owner == user.uid => {}
            Some(_) => return forbidden(format!("{} belongs to another user", bot.id)),
            None => {
                return forbidden(format!(
                    "{} was imported and is not claimed yet; ask the owner to claim it for you",
                    bot.id
                ));
            }
        }
    }
    if is_admin(config, user) {
        return Ok(());
    }
    let limits = &config.submit;
    let young = user
        .github_created
        .is_none_or(|created| created + u64::from(limits.min_account_days) * DAY > now);
    if young && !user.trusted {
        return forbidden(format!(
            "GitHub accounts younger than {} days need the owner's approval to submit; please \
             open an issue",
            limits.min_account_days
        ));
    }
    if submission.bot.is_none() && submission.owned_bots >= limits.max_bots {
        return forbidden(format!("at most {} bots per user", limits.max_bots));
    }
    if submission.bot.is_some() && submission.versions_last_day >= limits.versions_per_day {
        return forbidden(format!(
            "at most {} new versions of a bot per day",
            limits.versions_per_day
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{BotSource, BotState, MemStore, Reputation};

    const NOW: u64 = 1_792_843_200;

    #[test]
    fn tokens() {
        let random = [7u8; 32];
        let upload = new_token(TokenKind::Upload, &random);
        assert!(upload.starts_with("rsu_"));
        assert_eq!(upload.len(), 4 + 43);
        assert_eq!(token_kind(&upload), Some(TokenKind::Upload));
        let session = new_token(TokenKind::Session, &random);
        assert_eq!(token_kind(&session), Some(TokenKind::Session));
        assert_eq!(token_kind("rsu_short"), None);
        assert_eq!(token_kind(&format!("xyz_{}", &upload[4..])), None);
        assert_eq!(
            hash_token(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(hash_token(&upload), hash_token(&session));
    }

    fn store_with_user(state: UserState) -> MemStore {
        let store = MemStore::new();
        let user = upsert_user(
            &store,
            &GithubProfile {
                id: 583_231,
                login: "nhofsted".into(),
                name: Some("Nick".into()),
                created_at: Some(1_300_000_000),
            },
            NOW,
        )
        .unwrap();
        assert_eq!(user.uid, "g583231");
        store
            .set_reputation(
                "g583231",
                0,
                &Reputation {
                    state,
                    ..Reputation::default()
                },
            )
            .unwrap();
        store
    }

    #[test]
    fn upload_tokens_authenticate_and_revoke() {
        let store = store_with_user(UserState::New);
        let (token, item) = issue_token(
            &store,
            "g583231",
            TokenKind::Upload,
            "desktop",
            &[1; 32],
            NOW,
        )
        .unwrap();
        assert_eq!(item.expires, None);
        let ok = authenticate(&store, &token, NOW + 10).unwrap();
        assert_eq!(ok.user.uid, "g583231");
        assert_eq!(ok.renewed, None);
        // lu written at most daily.
        assert_eq!(store.get_token(&item.hash).unwrap().unwrap().last_used, NOW);
        authenticate(&store, &token, NOW + DAY).unwrap();
        assert_eq!(
            store.get_token(&item.hash).unwrap().unwrap().last_used,
            NOW + DAY
        );
        // Unknown and malformed tokens.
        let other = new_token(TokenKind::Upload, &[2; 32]);
        assert_eq!(
            authenticate(&store, &other, NOW).unwrap_err().code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            authenticate(&store, "Bearer x", NOW).unwrap_err().code,
            ErrorCode::Unauthorized
        );
        // Revoking by id, then "current".
        let (second, second_item) = issue_token(
            &store,
            "g583231",
            TokenKind::Upload,
            "laptop",
            &[3; 32],
            NOW,
        )
        .unwrap();
        revoke(&store, "g583231", item.id(), "").unwrap();
        assert!(authenticate(&store, &token, NOW).is_err());
        assert_eq!(
            revoke(&store, "g583231", "000000000000", "")
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        revoke(&store, "g583231", "current", &second_item.hash).unwrap();
        assert!(authenticate(&store, &second, NOW).is_err());
    }

    #[test]
    fn sessions_expire_and_renew() {
        let store = store_with_user(UserState::New);
        let (session, item) =
            issue_token(&store, "g583231", TokenKind::Session, "web", &[4; 32], NOW).unwrap();
        assert_eq!(item.expires, Some(NOW + SESSION_SECONDS));
        // Early use: no renewal.
        assert_eq!(
            authenticate(&store, &session, NOW + DAY).unwrap().renewed,
            None
        );
        // Less than 7 days left: renewed.
        let late = NOW + SESSION_SECONDS - 6 * DAY;
        let ok = authenticate(&store, &session, late).unwrap();
        assert_eq!(ok.renewed, Some(late + SESSION_SECONDS));
        assert_eq!(
            store.get_token(&item.hash).unwrap().unwrap().expires,
            Some(late + SESSION_SECONDS)
        );
        // Expired.
        let gone = late + SESSION_SECONDS;
        assert_eq!(
            authenticate(&store, &session, gone).unwrap_err().code,
            ErrorCode::Unauthorized
        );
        assert!(session_needs_renewal(NOW + DAY, NOW));
        assert!(!session_needs_renewal(NOW + 8 * DAY, NOW));
        assert!(!session_needs_renewal(NOW, NOW));
    }

    #[test]
    fn banned_users_tokens_fail() {
        let store = store_with_user(UserState::Banned);
        let (token, _) =
            issue_token(&store, "g583231", TokenKind::Upload, "x", &[5; 32], NOW).unwrap();
        assert_eq!(
            authenticate(&store, &token, NOW).unwrap_err().code,
            ErrorCode::Forbidden
        );
    }

    #[test]
    fn origin_check() {
        let site = "https://rumble.planethofstede.be";
        assert!(check_origin(Some(site), site).is_ok());
        assert!(check_origin(Some("https://evil.example"), site).is_err());
        assert!(check_origin(None, site).is_err());
        assert!(check_origin(Some("https://rumble.planethofstede.be.evil"), site).is_err());
    }

    #[test]
    fn upserts_follow_the_profile() {
        let store = store_with_user(UserState::New);
        let profile = GithubProfile {
            id: 583_231,
            login: "renamed".into(),
            name: None,
            created_at: Some(1_300_000_000),
        };
        let user = upsert_user(&store, &profile, NOW + 99).unwrap();
        assert_eq!(user.login, "renamed");
        assert_eq!(user.name, "");
        assert_eq!(user.created, NOW, "creation time kept");
        assert_eq!(store.get_user("g583231").unwrap().unwrap().login, "renamed");
        assert_eq!(user.github_created, Some(1_300_000_000));
    }

    #[test]
    fn admins_and_me() {
        let config = Config::default();
        let mut user = User {
            uid: "g4036752".into(),
            github_id: 4_036_752,
            created: NOW,
            ..User::default()
        };
        assert!(is_admin(&config, &user));
        user.github_id = 1;
        assert!(!is_admin(&config, &user));
        user.admin = true;
        assert!(is_admin(&config, &user));
        user.reputation.rep = 61;
        let me = me(&config, &user);
        assert_eq!(me.tier, crate::api::Tier::Verifier);
        assert_eq!(me.reputation, 61);
        assert_eq!(me.since, "2026-10-24T12:00:00Z");
        assert!(me.admin);
    }

    fn bot(owner: Option<&str>) -> BotItem {
        BotItem {
            id: "a.B".into(),
            author: "a".into(),
            owner: owner.map(str::to_string),
            homepage: None,
            description: None,
            divisions: vec![],
            versions: vec![],
            current: None,
            state: BotState::Active,
            created: 0,
            source: BotSource::Submit,
        }
    }

    #[test]
    fn submission_limits() {
        let config = Config::default();
        let old_account = NOW - 365 * DAY;
        let user = User {
            uid: "g1".into(),
            github_id: 1,
            github_created: Some(old_account),
            ..User::default()
        };
        let new_bot = Submission {
            bot: None,
            owned_bots: 4,
            versions_last_day: 0,
            import: false,
        };
        assert!(check_submission(&config, &user, &new_bot, NOW).is_ok());
        let too_many = Submission {
            owned_bots: 5,
            ..new_bot
        };
        assert_eq!(
            check_submission(&config, &user, &too_many, NOW)
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );

        let own = bot(Some("g1"));
        let update = Submission {
            bot: Some(&own),
            owned_bots: 5,
            versions_last_day: 4,
            import: false,
        };
        assert!(
            check_submission(&config, &user, &update, NOW).is_ok(),
            "max bots is for new bots"
        );
        let busy = Submission {
            versions_last_day: 5,
            ..update
        };
        assert!(check_submission(&config, &user, &busy, NOW).is_err());

        let foreign = bot(Some("g2"));
        let theirs = Submission {
            bot: Some(&foreign),
            ..update
        };
        assert!(check_submission(&config, &user, &theirs, NOW).is_err());
        let unclaimed = bot(None);
        let imported = Submission {
            bot: Some(&unclaimed),
            ..update
        };
        assert!(
            check_submission(&config, &user, &imported, NOW)
                .unwrap_err()
                .message
                .contains("claim")
        );

        // Young accounts need the owner's approval; trusted ones don't.
        let mut young = User {
            github_created: Some(NOW - 29 * DAY),
            ..user.clone()
        };
        assert!(check_submission(&config, &young, &new_bot, NOW).is_err());
        young.github_created = None;
        assert!(check_submission(&config, &young, &new_bot, NOW).is_err());
        young.trusted = true;
        assert!(check_submission(&config, &young, &new_bot, NOW).is_ok());
        let thirty = User {
            github_created: Some(NOW - 30 * DAY),
            ..user.clone()
        };
        assert!(check_submission(&config, &thirty, &new_bot, NOW).is_ok());

        // Admins and imports are exempt from the limits.
        let admin = User {
            admin: true,
            github_created: None,
            ..user.clone()
        };
        assert!(check_submission(&config, &admin, &too_many, NOW).is_ok());
        assert!(check_submission(&config, &admin, &busy, NOW).is_ok());
        assert!(
            check_submission(&config, &admin, &theirs, NOW).is_err(),
            "not ownership"
        );
        let import = Submission {
            import: true,
            ..theirs
        };
        assert!(check_submission(&config, &young, &import, NOW).is_ok());

        // Banned users can't submit.
        let mut banned = user.clone();
        banned.reputation.state = UserState::Banned;
        assert!(check_submission(&config, &banned, &new_bot, NOW).is_err());
    }

    #[test]
    fn counting_versions_per_day() {
        let version = |bot: &str, created: u64| VersionItem {
            vid: 1,
            bot: bot.into(),
            version: "1".into(),
            sha256: String::new(),
            size: 0,
            codesize: None,
            class: crate::ids::WeightClass::Mega,
            divisions: vec![],
            state: crate::store::VersionState::Probation,
            probation_battles: 0,
            probation_failures: 0,
            class_name: String::new(),
            created,
            retired: None,
            url: None,
            colors: None,
        };
        let versions = [
            version("a.B", NOW - 10),
            version("a.B", NOW - DAY + 1),
            version("a.B", NOW - DAY),
            version("c.D", NOW),
        ];
        assert_eq!(versions_last_day(&versions, "a.B", NOW), 2);
    }
}
