// SPDX-License-Identifier: EPL-2.0

//! The `api` router (spec §5.4): every `/api/v1` endpoint over the core, with what an
//! instance keeps in memory: the configuration (re-read at most every 5 minutes), the plans
//! (ETag-checked at most every 60 s) with their samplers, the 5-minute token cache, and the
//! rate limits. [`App::handle`] is synchronous and runs on a blocking thread.

use crate::fetch::Fetcher;
use crate::github::{Github, GithubError};
use crate::http::{
    MAX_BODY, OAUTH_COOKIE, Request, Response, SESSION_COOKIE, client_release, compare_releases,
    rate_limited,
};
use crate::limits::Limiter;
use crate::s3::{Bucket, Fetched, IMMUTABLE, JAR_TYPE, Objects, PutSpec, TRACK_TYPE};
use crate::{DETERMINISTIC_RULES, ENGINE_VERSION};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rumble_core::api::{
    AlertInfo, AlertsResponse, ApiError, AuthTokenRequest, AuthTokenResponse, Battlefield,
    BotRequest, BotStateResponse, BotsResponse, ClaimRequest, ErrorCode, FailedSlot, ImportRequest,
    Info, InfoDivision, MeUpdate, OkResponse, RECORDINGS_DONE_MAX_SLOTS, RecordingsDoneRequest,
    RecordingsDoneResponse, RemoveRequest, ResultsRequest, RevokeRequest, SlotFailure,
    SubmitRequest, SubmitSource, TokenInfo, TokensResponse, UploadUrlRequest, UploadUrlResponse,
    UserUpdateRequest, WorkQuery,
};
use rumble_core::auth::{self, Authenticated, SESSION_SECONDS};
use rumble_core::bots;
use rumble_core::config::Config;
use rumble_core::flags;
use rumble_core::ids::{Division, parse_uid};
use rumble_core::jar;
use rumble_core::plan::Plan;
use rumble_core::results::{self, Presigned, Presigner, ResultsContext};
use rumble_core::slots::{self, SlotId, is_sha256_hex};
use rumble_core::store::{Store, TokenKind, User, UserPatch, rfc3339};
use rumble_core::token::{self, Keys};
use rumble_core::work::WorkServer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};

/// The configuration is re-read at most this often (§3).
pub const CONFIG_MS: u64 = 5 * 60_000;
/// Plans are checked for a new ETag at most this often (§10.5).
pub const PLAN_MS: u64 = 60_000;
/// Token lookups are cached this long (§7.3).
pub const AUTH_MS: u64 = 5 * 60_000;
/// A presigned jar upload is valid this long (§5.4).
pub const UPLOAD_SECONDS: u64 = 15 * 60;
/// The OAuth state and its cookie live this long (§7.1).
pub const OAUTH_SECONDS: u64 = 600;

pub const CONFIG_KEY: &str = "state/config.json";

/// The S3 key of a division's plan.
pub fn plan_key(engine: u32, division: Division) -> String {
    format!("state/e{engine}/{}/plan.json.gz", division.as_str())
}

/// The S3 key of a user's jar upload.
pub fn upload_key(uid: &str, sha256: &str) -> String {
    format!("uploads/{uid}/{sha256}.jar")
}

pub fn jar_key(sha256: &str) -> String {
    format!("jars/{sha256}.jar")
}

/// Deployment facts (environment variables, §14.2).
#[derive(Clone, Debug)]
pub struct Settings {
    /// `https://<domain>`: the CSRF origin, redirects.
    pub site_origin: String,
    pub github_client_id: String,
}

/// The I/O the router uses.
pub struct Services {
    pub store: Box<dyn Store + Send + Sync>,
    pub objects: Box<dyn Objects>,
    pub github: Box<dyn Github>,
    pub fetcher: Box<dyn Fetcher>,
    /// Milliseconds since the epoch.
    pub clock: Box<dyn Fn() -> u64 + Send + Sync>,
    pub random: Box<dyn Fn() -> [u8; 32] + Send + Sync>,
}

struct CachedAuth {
    auth: Authenticated,
    at_ms: u64,
}

struct Inner {
    config: Config,
    config_etag: Option<String>,
    config_checked: Option<u64>,
    work: WorkServer,
    /// Per division: the plan's key and ETag, and when it was checked.
    plans: BTreeMap<Division, (String, String)>,
    plans_checked: BTreeMap<Division, u64>,
    auth: HashMap<String, CachedAuth>,
    limiter: Limiter,
}

pub struct App {
    services: Services,
    keys: Keys,
    settings: Settings,
    inner: Mutex<Inner>,
}

/// Who calls.
struct Caller {
    auth: Authenticated,
    via_cookie: bool,
}

impl Caller {
    fn user(&self) -> &User {
        &self.auth.user
    }
}

#[derive(Serialize, Deserialize)]
struct OauthState {
    n: String,
    r: String,
    exp: u64,
}

type Handled = Result<Response, ApiError>;

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, message)
}

fn busy(message: impl std::fmt::Display) -> ApiError {
    eprintln!("ERROR {message}");
    ApiError::new(ErrorCode::Busy, "the server is busy; retry later")
}

fn github_error(e: GithubError) -> ApiError {
    match e {
        GithubError::Rejected(why) => ApiError::new(
            ErrorCode::Unauthorized,
            format!("GitHub did not accept the sign-in ({why})"),
        ),
        GithubError::Unavailable(why) => busy(format!("GitHub: {why}")),
    }
}

fn base_config() -> Config {
    Config {
        engine_version: ENGINE_VERSION,
        ..Config::default()
    }
}

/// The configuration from `state/config.json`'s overrides; `engineVersion` defaults to
/// this build's.
pub fn config_from(overrides: &Value) -> Result<Config, String> {
    let mut overrides = overrides.clone();
    if let Some(object) = overrides.as_object_mut() {
        object
            .entry("engineVersion")
            .or_insert(Value::from(ENGINE_VERSION));
    }
    Config::with_overrides(&overrides)
}

/// A relative path to return to after sign-in; anything else becomes `/` (§7.1).
pub fn safe_return(path: Option<&str>) -> String {
    match path {
        Some(p)
            if p.starts_with('/')
                && !p.starts_with("//")
                && p.len() <= 512
                && !p.contains('\\')
                && !p.chars().any(char::is_control) =>
        {
            p.to_string()
        }
        _ => "/".to_string(),
    }
}

struct RecordingPresigner<'a>(&'a dyn Objects);

impl Presigner for RecordingPresigner<'_> {
    fn put_recording(&self, key: &str, sha256_b64: &str, size: u64, now: u64) -> Presigned {
        let spec = PutSpec {
            size,
            sha256_b64: sha256_b64.to_string(),
            content_type: TRACK_TYPE.to_string(),
            cache_control: Some(IMMUTABLE.to_string()),
        };
        match self
            .0
            .presign_put(Bucket::Jars, key, &spec, slots::CLAIM_SECONDS, now)
        {
            Ok(presigned) => presigned,
            Err(e) => {
                eprintln!("ERROR presigning {key}: {e}");
                Presigned {
                    url: String::new(),
                    headers: BTreeMap::new(),
                    expires: rfc3339(now),
                }
            }
        }
    }
}

impl App {
    pub fn new(services: Services, keys: Keys, settings: Settings) -> App {
        App {
            services,
            keys,
            settings,
            inner: Mutex::new(Inner {
                config: base_config(),
                config_etag: None,
                config_checked: None,
                work: WorkServer::new(),
                plans: BTreeMap::new(),
                plans_checked: BTreeMap::new(),
                auth: HashMap::new(),
                limiter: Limiter::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn store(&self) -> &dyn Store {
        self.services.store.as_ref()
    }

    fn random(&self) -> [u8; 32] {
        (self.services.random)()
    }

    /// Handles one request. Never panics on bad input; errors are §5.2 bodies.
    pub fn handle(&self, request: &Request) -> Response {
        let now_ms = (self.services.clock)();
        match self.route(request, now_ms) {
            Ok(response) => response,
            Err(error) => Response::error(&error),
        }
    }

    // ---- instance state ----

    /// The configuration, re-read when older than [`CONFIG_MS`].
    pub fn config(&self, now_ms: u64) -> Config {
        let mut inner = self.lock();
        let due = inner
            .config_checked
            .is_none_or(|at| now_ms.saturating_sub(at) >= CONFIG_MS);
        if due {
            let etag = inner.config_etag.clone();
            match self
                .services
                .objects
                .get(Bucket::State, CONFIG_KEY, etag.as_deref())
            {
                Ok(Fetched::NotModified) => {}
                Ok(Fetched::Missing) => {
                    inner.config = base_config();
                    inner.config_etag = None;
                }
                Ok(Fetched::Found { bytes, etag }) => {
                    match serde_json::from_slice::<Value>(&bytes)
                        .map_err(|e| e.to_string())
                        .and_then(|v| config_from(&v))
                    {
                        Ok(config) => {
                            inner.config = config;
                            inner.config_etag = Some(etag);
                        }
                        Err(e) => eprintln!("ERROR {CONFIG_KEY}: {e}; keeping the previous config"),
                    }
                }
                Err(e) => eprintln!("ERROR {e}; keeping the previous config"),
            }
            inner.config_checked = Some(now_ms);
        }
        inner.config.clone()
    }

    /// Reloads the plans of `divisions` whose last check is older than [`PLAN_MS`].
    fn refresh_plans(&self, inner: &mut Inner, engine: u32, divisions: &[Division], now_ms: u64) {
        for &division in divisions {
            let due = inner
                .plans_checked
                .get(&division)
                .is_none_or(|&at| now_ms.saturating_sub(at) >= PLAN_MS);
            if !due {
                continue;
            }
            let key = plan_key(engine, division);
            let etag = inner
                .plans
                .get(&division)
                .filter(|(k, _)| *k == key)
                .map(|(_, e)| e.clone());
            match self
                .services
                .objects
                .get(Bucket::State, &key, etag.as_deref())
            {
                Ok(Fetched::NotModified) => {}
                Ok(Fetched::Missing) => {
                    inner.work.remove_plan(division);
                    inner.plans.remove(&division);
                }
                Ok(Fetched::Found { bytes, etag }) => match Plan::from_bytes(&bytes) {
                    Ok(plan) if plan.engine == engine && plan.division == division => {
                        inner.work.set_plan(plan);
                        inner.plans.insert(division, (key, etag));
                    }
                    Ok(_) => eprintln!("ERROR {key}: engine or division doesn't match"),
                    Err(e) => eprintln!("ERROR {key}: {e}"),
                },
                Err(e) => eprintln!("ERROR {e}"),
            }
            inner.plans_checked.insert(division, now_ms);
        }
    }

    /// Authenticates the caller (§7.3): `X-Rumble-Token` or the session cookie (only the
    /// cookie if `session_only`), with the instance's 5-minute cache, then the per-token
    /// rate limit and, for cookie POSTs, the CSRF origin check.
    fn caller(
        &self,
        request: &Request,
        config: &Config,
        now_ms: u64,
        session_only: bool,
        limit: Limit,
    ) -> Result<Result<Caller, Response>, ApiError> {
        let header = (!session_only)
            .then(|| request.get("x-rumble-token"))
            .flatten();
        let (token, via_cookie) = match (header, request.cookie(SESSION_COOKIE)) {
            (Some(token), _) => (token, false),
            (None, Some(cookie)) => (cookie, true),
            (None, None) => {
                return Err(ApiError::new(ErrorCode::Unauthorized, "sign in first"));
            }
        };
        let hash = auth::hash_token(token);
        let now = now_ms / 1000;
        let cached = {
            let inner = self.lock();
            inner.auth.get(&hash).and_then(|c| {
                let fresh = now_ms.saturating_sub(c.at_ms) < AUTH_MS
                    && c.auth.token.expires.is_none_or(|e| e > now);
                fresh.then(|| c.auth.clone())
            })
        };
        let auth = match cached {
            Some(auth) => Authenticated {
                renewed: None,
                ..auth
            },
            None => {
                let auth = match auth::authenticate(self.store(), token, now) {
                    Ok(auth) => auth,
                    Err(e) => {
                        // Unknown tokens are limited like any other (they cost a read).
                        if let Err(retry) = self.limited(&hash, config, limit, now_ms) {
                            return Ok(Err(rate_limited(retry)));
                        }
                        return Err(e);
                    }
                };
                self.lock().auth.insert(
                    hash.clone(),
                    CachedAuth {
                        auth: auth.clone(),
                        at_ms: now_ms,
                    },
                );
                auth
            }
        };
        // Admins (the importer) are exempt from the per-token limits (§5.3).
        if !auth::is_admin(config, &auth.user)
            && let Err(retry) = self.limited(&hash, config, limit, now_ms)
        {
            return Ok(Err(rate_limited(retry)));
        }
        if via_cookie && request.method == "POST" {
            auth::check_origin(request.get("origin"), &self.settings.site_origin)?;
        }
        Ok(Ok(Caller { auth, via_cookie }))
    }

    /// Takes one token from the caller's bucket for this kind of call.
    fn limited(&self, hash: &str, config: &Config, limit: Limit, now_ms: u64) -> Result<(), u64> {
        let mut inner = self.lock();
        match limit {
            Limit::Work => {
                inner
                    .limiter
                    .interval(&format!("w:{hash}"), config.rate.work_seconds, now_ms)
            }
            Limit::Results => {
                inner
                    .limiter
                    .interval(&format!("r:{hash}"), config.rate.results_seconds, now_ms)
            }
            Limit::Other => inner.limiter.check(&format!("o:{hash}"), 2.0, 10.0, now_ms),
        }
    }

    /// Drops cached lookups of a user (after a revocation or an admin change).
    fn forget_user(&self, uid: &str) {
        self.lock().auth.retain(|_, c| c.auth.user.uid != uid);
    }

    /// A fresh copy of the caller's user.
    fn fresh_user(&self, caller: &Caller) -> Result<User, ApiError> {
        self.store()
            .get_user(&caller.user().uid)?
            .ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "unknown user"))
    }

    // ---- routing ----

    fn route(&self, request: &Request, now_ms: u64) -> Handled {
        if request.body.len() > MAX_BODY {
            return Err(ApiError::new(ErrorCode::TooLarge, "the body is over 1 MiB"));
        }
        let Some(path) = request.path.strip_prefix("/api/v1/") else {
            return Err(ApiError::new(ErrorCode::NotFound, "no such endpoint"));
        };
        let config = self.config(now_ms);
        if let Some(release) = request.get("x-rumble-client").and_then(client_release)
            && compare_releases(release, &config.min_client) == Some(std::cmp::Ordering::Less)
        {
            return Err(ApiError::new(
                ErrorCode::ClientOutdated,
                format!(
                    "robocode-s {release} is too old for the rumble (at least {} is needed); \
                     download a new release from {}/download",
                    config.min_client, self.settings.site_origin
                ),
            ));
        }
        let method = request.method.as_str();
        if method != "GET" && method != "POST" {
            return Err(ApiError::new(ErrorCode::NotFound, "no such endpoint"));
        }

        // Public endpoints; auth/* is rate-limited per viewer IP.
        if path == "info" && method == "GET" {
            return Ok(Response::ok(&self.info(&config)));
        }
        if path.starts_with("auth/") {
            let ip = request.viewer_ip();
            if let Err(retry) = self
                .lock()
                .limiter
                .check(&format!("ip:{ip}"), 1.0, 20.0, now_ms)
            {
                return Ok(rate_limited(retry));
            }
            match (method, path) {
                ("GET", "auth/login") => return self.login(request, now_ms),
                ("GET", "auth/callback") => return self.callback(request, now_ms),
                ("POST", "auth/token") => return self.auth_token(request, &config, now_ms),
                ("POST", "auth/logout") => {}
                _ => return Err(ApiError::new(ErrorCode::NotFound, "no such endpoint")),
            }
        }

        let known = matches!(
            (method, path),
            ("POST", "auth/logout")
                | ("GET", "me")
                | ("POST", "me")
                | ("GET", "me/tokens")
                | ("POST", "me/tokens/revoke")
                | ("GET", "me/bots")
                | ("GET", "work")
                | ("POST", "results")
                | ("POST", "recordings/done")
                | ("POST", "bots/upload-url")
                | ("POST", "bots/submit")
                | ("POST", "bots/retire")
                | ("POST", "admin/jars/upload-url")
                | ("POST", "admin/bots/import")
                | ("POST", "admin/bots/claim")
                | ("POST", "admin/bots/remove")
                | ("POST", "admin/users/update")
                | ("GET", "admin/alerts")
                | ("GET", "admin/config")
                | ("POST", "admin/config")
        );
        if !known {
            return Err(ApiError::new(ErrorCode::NotFound, "no such endpoint"));
        }
        let limit = match path {
            "work" => Limit::Work,
            "results" => Limit::Results,
            _ => Limit::Other,
        };
        let caller = match self.caller(request, &config, now_ms, path == "auth/logout", limit)? {
            Ok(caller) => caller,
            Err(limited) => return Ok(limited),
        };
        if path.starts_with("admin/") && !auth::is_admin(&config, caller.user()) {
            return Err(ApiError::new(ErrorCode::Forbidden, "admins only"));
        }
        let response = match (method, path) {
            ("POST", "auth/logout") => self.logout(&caller),
            ("GET", "me") => self.me(&caller, &config),
            ("POST", "me") => self.update_me(request, &caller, &config),
            ("GET", "me/tokens") => self.tokens(&caller, now_ms),
            ("POST", "me/tokens/revoke") => self.revoke(request, &caller),
            ("GET", "me/bots") => self.my_bots(&caller),
            ("GET", "work") => self.work(request, &caller, &config, now_ms),
            ("POST", "results") => self.results(request, &caller, &config, now_ms),
            ("POST", "recordings/done") => self.recordings_done(request, &caller, &config),
            ("POST", "bots/upload-url") => {
                self.upload_url(request, &caller, &config, now_ms, false)
            }
            ("POST", "bots/submit") => self.submit(request, &caller, &config, now_ms),
            ("POST", "bots/retire") => self.retire(request, &caller, &config, now_ms),
            ("POST", "admin/jars/upload-url") => {
                self.upload_url(request, &caller, &config, now_ms, true)
            }
            ("POST", "admin/bots/import") => self.import(request, &caller, &config, now_ms),
            ("POST", "admin/bots/claim") => self.claim(request),
            ("POST", "admin/bots/remove") => self.remove(request, now_ms),
            ("POST", "admin/users/update") => self.update_user(request, &config, now_ms),
            ("GET", "admin/alerts") => self.alerts(),
            ("GET", "admin/config") => self.get_config(),
            ("POST", "admin/config") => self.set_config(request),
            _ => Err(ApiError::new(ErrorCode::NotFound, "no such endpoint")),
        }?;
        // A session renewed in its last week gets its cookie again (§7.1).
        Ok(match (caller.via_cookie, caller.auth.renewed, path) {
            (true, Some(_), p) if p != "auth/logout" => {
                let cookie = request
                    .cookie(SESSION_COOKIE)
                    .unwrap_or_default()
                    .to_string();
                response.with_cookie(SESSION_COOKIE, &cookie, SESSION_SECONDS)
            }
            _ => response,
        })
    }

    // ---- public ----

    fn info(&self, config: &Config) -> Info {
        Info {
            api: 1,
            engine_version: config.engine_version,
            min_client: config.min_client.clone(),
            paused: config.paused,
            github_client_id: self.settings.github_client_id.clone(),
            deterministic_rules: DETERMINISTIC_RULES,
            divisions: Division::ALL
                .iter()
                .map(|&d| {
                    let division = config.divisions.get(d);
                    InfoDivision {
                        id: d,
                        rounds: division.rounds,
                        battlefield: Battlefield {
                            width: division.width,
                            height: division.height,
                        },
                        robots: division.robots,
                    }
                })
                .collect(),
            downloads: format!("{}/download", self.settings.site_origin),
        }
    }

    fn redirect_uri(&self) -> String {
        format!("{}/api/v1/auth/callback", self.settings.site_origin)
    }

    fn login(&self, request: &Request, now_ms: u64) -> Handled {
        let now = now_ms / 1000;
        let random = self.random();
        let nonce = URL_SAFE_NO_PAD.encode(&random[..16]);
        let state = token::sign_state(
            &OauthState {
                n: nonce.clone(),
                r: safe_return(request.param("return").as_deref()),
                exp: now + OAUTH_SECONDS,
            },
            &self.keys,
        )
        .map_err(busy)?;
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", &self.settings.github_client_id)
            .append_pair("redirect_uri", &self.redirect_uri())
            .append_pair("state", &state)
            .append_pair("allow_signup", "true")
            .finish();
        Ok(
            Response::redirect(&format!("https://github.com/login/oauth/authorize?{query}"))
                .with_cookie(OAUTH_COOKIE, &nonce, OAUTH_SECONDS),
        )
    }

    fn callback(&self, request: &Request, now_ms: u64) -> Handled {
        let now = now_ms / 1000;
        let (Some(code), Some(state)) = (request.param("code"), request.param("state")) else {
            return Err(bad_request("code and state are required"));
        };
        let state: OauthState = token::verify_state(&state, &self.keys, now).map_err(|e| {
            bad_request(format!(
                "the sign-in expired or is invalid ({e}); try again"
            ))
        })?;
        if request.cookie(OAUTH_COOKIE) != Some(state.n.as_str()) {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "the sign-in was started in another browser; try again",
            ));
        }
        let github = &self.services.github;
        let token = github
            .exchange_code(&code, &self.redirect_uri())
            .map_err(github_error)?;
        let profile = github.user(&token).map_err(github_error);
        if let Err(e) = github.delete_token(&token) {
            eprintln!("WARN deleting a GitHub token: {e}");
        }
        let user = auth::upsert_user(self.store(), &profile?, now)?;
        if user.reputation.state == rumble_core::store::UserState::Banned {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "this account is banned",
            ));
        }
        let (session, _) = auth::issue_token(
            self.store(),
            &user.uid,
            TokenKind::Session,
            "web",
            &self.random(),
            now,
        )?;
        Ok(
            Response::redirect(&format!("{}{}", self.settings.site_origin, state.r))
                .with_cookie(SESSION_COOKIE, &session, SESSION_SECONDS)
                .with_cookie(OAUTH_COOKIE, "", 0),
        )
    }

    fn auth_token(&self, request: &Request, config: &Config, now_ms: u64) -> Handled {
        let now = now_ms / 1000;
        let body: AuthTokenRequest = request.body_json()?;
        let github = &self.services.github;
        // Only tokens issued to our OAuth app pass the application API (§7.2).
        let id = github
            .check_token(&body.github_token)
            .map_err(github_error)?;
        let profile = github.user(&body.github_token).map_err(github_error);
        if let Err(e) = github.delete_token(&body.github_token) {
            eprintln!("WARN deleting a GitHub token: {e}");
        }
        let profile = profile?;
        if profile.id != id {
            return Err(ApiError::new(
                ErrorCode::Unauthorized,
                "the GitHub token is inconsistent",
            ));
        }
        let user = auth::upsert_user(self.store(), &profile, now)?;
        if user.reputation.state == rumble_core::store::UserState::Banned {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "this account is banned",
            ));
        }
        let label = body.label.trim();
        let label = if label.is_empty() { "client" } else { label };
        let (token, _) = auth::issue_token(
            self.store(),
            &user.uid,
            TokenKind::Upload,
            label,
            &self.random(),
            now,
        )?;
        Ok(Response::ok(&AuthTokenResponse {
            token,
            user: auth::me(config, &user),
        }))
    }

    // ---- the user ----

    fn logout(&self, caller: &Caller) -> Handled {
        let token = &caller.auth.token;
        self.store().delete_token(&token.uid, &token.hash)?;
        self.lock().auth.remove(&token.hash);
        Ok(Response::empty(204).with_cookie(SESSION_COOKIE, "", 0))
    }

    fn me(&self, caller: &Caller, config: &Config) -> Handled {
        Ok(Response::ok(&auth::me(config, &self.fresh_user(caller)?)))
    }

    fn update_me(&self, request: &Request, caller: &Caller, config: &Config) -> Handled {
        let body: MeUpdate = request.body_json()?;
        if let Some(country) = &body.country
            && !(country.is_empty()
                || country == flags::ANCAP_FLAG
                || (country.len() == 2 && country.bytes().all(|b| b.is_ascii_lowercase())))
        {
            return Err(bad_request(
                "country: two lower-case letters (ISO 3166-1, or un), ancap, or \"\"",
            ));
        }
        let patch = UserPatch {
            country: body.country,
            hidden: body.hidden,
            ..UserPatch::default()
        };
        self.store().patch_user(&caller.user().uid, &patch)?;
        self.forget_user(&caller.user().uid);
        Ok(Response::ok(&auth::me(config, &self.fresh_user(caller)?)))
    }

    fn tokens(&self, caller: &Caller, now_ms: u64) -> Handled {
        let now = now_ms / 1000;
        let tokens = self
            .store()
            .list_tokens(&caller.user().uid)?
            .into_iter()
            .filter(|t| t.expires.is_none_or(|e| e > now))
            .map(|t| TokenInfo {
                id: t.id().to_string(),
                kind: t.kind.as_str().to_string(),
                label: t.label.clone(),
                created: rfc3339(t.created),
                last_used: rfc3339(t.last_used),
                expires: t.expires.map(rfc3339),
                current: t.hash == caller.auth.token.hash,
            })
            .collect();
        Ok(Response::ok(&TokensResponse { tokens }))
    }

    fn revoke(&self, request: &Request, caller: &Caller) -> Handled {
        let body: RevokeRequest = request.body_json()?;
        auth::revoke(
            self.store(),
            &caller.user().uid,
            &body.id,
            &caller.auth.token.hash,
        )?;
        // Effective at once in this instance (§7.3).
        let id = body.id.clone();
        let current = caller.auth.token.hash.clone();
        self.lock().auth.retain(|hash, c| {
            !(c.auth.user.uid == caller.user().uid
                && (hash.starts_with(&id) || (id == "current" && *hash == current)))
        });
        Ok(Response::ok(&OkResponse { ok: true }))
    }

    fn my_bots(&self, caller: &Caller) -> Handled {
        let user = self.fresh_user(caller)?;
        Ok(Response::ok(&BotsResponse {
            bots: bots::list_bots(self.store(), &user)?,
        }))
    }

    // ---- work and results ----

    fn work(&self, request: &Request, caller: &Caller, config: &Config, now_ms: u64) -> Handled {
        let query = WorkQuery::parse(&request.query).map_err(bad_request)?;
        let random = self.random();
        let mut inner = self.lock();
        self.refresh_plans(&mut inner, config.engine_version, &query.divisions, now_ms);
        let served = inner.work.serve(
            self.store(),
            config,
            &self.keys,
            caller.user(),
            &query,
            now_ms,
            &random,
        )?;
        Ok(match served {
            Some(work) => Response::ok(&work),
            None => {
                let wait = if config.paused { "300" } else { "60" };
                Response::empty(204).with_header("retry-after", wait)
            }
        })
    }

    fn results(&self, request: &Request, caller: &Caller, config: &Config, now_ms: u64) -> Handled {
        let body: ResultsRequest = request.body_json()?;
        let random = self.random();
        let outcome = {
            let mut inner = self.lock();
            self.refresh_plans(&mut inner, config.engine_version, &Division::ALL, now_ms);
            let work = &inner.work;
            let top = |division: Division, vid: u32| work.is_top(division, vid);
            let slot_pair =
                |division: Division, lo: u32, hi: u32| work.is_slot_pair(division, lo, hi);
            let presigner = RecordingPresigner(self.services.objects.as_ref());
            let ctx = ResultsContext {
                config,
                keys: &self.keys,
                top: &top,
                slot_pair: &slot_pair,
                presigner: &presigner,
            };
            results::apply_results(self.store(), &ctx, caller.user(), &body, now_ms, &random)?
        };
        for line in &outcome.log {
            println!("{line}");
        }
        let battles = outcome.response.user.battles;
        if let Some(cached) = self.lock().auth.get_mut(&caller.auth.token.hash) {
            cached.auth.user.battles = battles;
        }
        Ok(Response::ok(&outcome.response))
    }

    fn recordings_done(&self, request: &Request, caller: &Caller, config: &Config) -> Handled {
        let body: RecordingsDoneRequest = request.body_json()?;
        if body.slots.len() > RECORDINGS_DONE_MAX_SLOTS {
            return Err(bad_request(format!(
                "at most {RECORDINGS_DONE_MAX_SLOTS} slots"
            )));
        }
        let payload = token::verify_signature(&body.batch, &self.keys)
            .map_err(|e| bad_request(format!("batch: {e}")))?;
        if payload.uid != caller.user().uid {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "the batch was issued to another user",
            ));
        }
        let engine = config.engine_version;
        if payload.e != engine {
            return Err(ApiError::new(
                ErrorCode::EngineMismatch,
                format!(
                    "the rumble ranks engine version {engine}; this batch is for engine {}",
                    payload.e
                ),
            ));
        }
        let store = self.store();
        let mut versions: HashMap<u32, Option<(String, String)>> = HashMap::new();
        let mut filled = Vec::new();
        let mut failed = Vec::new();
        for slot in &body.slots {
            let key = match SlotId::parse(slot) {
                Ok(id) => {
                    let vids = match id.group {
                        slots::Group::Pair { lo, hi } => vec![lo, hi],
                        slots::Group::Version(vid) => vec![vid],
                    };
                    for vid in vids {
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            versions.entry(vid)
                        {
                            entry.insert(store.get_version(vid)?.map(|v| (v.bot, v.sha256)));
                        }
                    }
                    slots::recording_key(engine, &id, &|vid| versions.get(&vid).cloned().flatten())
                }
                Err(_) => None,
            };
            let head = match &key {
                Some(key) => self
                    .services
                    .objects
                    .head_checksum(Bucket::Jars, key)
                    .map_err(busy)?
                    .map(|checksum| checksum.unwrap_or_default()),
                None => None,
            };
            if key.is_none() {
                failed.push(FailedSlot {
                    slot: slot.clone(),
                    error: SlotFailure::Missing,
                });
                continue;
            }
            match slots::confirm(store, config, engine, &payload.id, slot, head.as_deref())? {
                Ok(()) => filled.push(slot.clone()),
                Err(error) => failed.push(FailedSlot {
                    slot: slot.clone(),
                    error,
                }),
            }
        }
        Ok(Response::ok(&RecordingsDoneResponse { filled, failed }))
    }

    // ---- bots ----

    fn upload_url(
        &self,
        request: &Request,
        caller: &Caller,
        config: &Config,
        now_ms: u64,
        admin: bool,
    ) -> Handled {
        let body: UploadUrlRequest = request.body_json()?;
        if !is_sha256_hex(&body.sha256) {
            return Err(bad_request("sha256: 64 lower-case hex digits"));
        }
        if body.size == 0 {
            return Err(bad_request("size: at least 1"));
        }
        if body.size > config.jar.max_bytes {
            return Err(ApiError::new(
                ErrorCode::TooLarge,
                format!("the jar is over {} bytes", config.jar.max_bytes),
            ));
        }
        let objects = &self.services.objects;
        if admin
            && objects
                .exists(Bucket::Jars, &jar_key(&body.sha256))
                .map_err(busy)?
        {
            return Ok(Response::ok(&UploadUrlResponse {
                exists: true,
                ..UploadUrlResponse::default()
            }));
        }
        let checksum = slots::hex_to_base64(&body.sha256).expect("checked above");
        let presigned = objects
            .presign_put(
                Bucket::State,
                &upload_key(&caller.user().uid, &body.sha256),
                &PutSpec {
                    size: body.size,
                    sha256_b64: checksum,
                    content_type: JAR_TYPE.to_string(),
                    cache_control: None,
                },
                UPLOAD_SECONDS,
                now_ms / 1000,
            )
            .map_err(busy)?;
        Ok(Response::ok(&UploadUrlResponse {
            url: presigned.url,
            headers: presigned.headers,
            expires: presigned.expires,
            exists: false,
        }))
    }

    /// The bytes of an upload (`uploads/<uid>/<sha>.jar`), checked against its hash.
    fn uploaded(&self, uid: &str, sha256: &str) -> Result<Vec<u8>, ApiError> {
        match self
            .services
            .objects
            .get(Bucket::State, &upload_key(uid, sha256), None)
            .map_err(busy)?
        {
            Fetched::Found { bytes, .. } if jar::sha256_hex(&bytes) == sha256 => Ok(bytes),
            Fetched::Found { .. } => Err(bad_request("the upload doesn't match its sha256")),
            _ => Err(bad_request(
                "no upload with this sha256: upload the jar first",
            )),
        }
    }

    /// Puts the jar into `jars/<sha>.jar` unless it is there.
    fn publish_jar(&self, sha256: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let objects = &self.services.objects;
        let key = jar_key(sha256);
        if !objects.exists(Bucket::Jars, &key).map_err(busy)? {
            objects
                .put(Bucket::Jars, &key, bytes, JAR_TYPE, Some(IMMUTABLE))
                .map_err(busy)?;
        }
        Ok(())
    }

    fn submit(&self, request: &Request, caller: &Caller, config: &Config, now_ms: u64) -> Handled {
        let body: SubmitRequest = request.body_json()?;
        let uid = caller.user().uid.clone();
        let (bytes, source_url) = match body.source {
            SubmitSource::Upload => {
                let sha = body
                    .sha256
                    .as_deref()
                    .filter(|s| is_sha256_hex(s))
                    .ok_or_else(|| bad_request("sha256: 64 lower-case hex digits"))?;
                (self.uploaded(&uid, sha)?, None)
            }
            SubmitSource::Url => {
                let url = body
                    .url
                    .as_deref()
                    .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
                    .ok_or_else(|| bad_request("url: an http or https URL"))?;
                let bytes = self
                    .services
                    .fetcher
                    .fetch(url, config.jar.max_bytes)
                    .map_err(|e| {
                        if e.starts_with("too large") {
                            ApiError::new(ErrorCode::TooLarge, e)
                        } else {
                            bad_request(e)
                        }
                    })?;
                (bytes, Some(url.to_string()))
            }
        };
        let info = jar::validate(&bytes, config)
            .map_err(|messages| bad_request(format!("invalid ({})", messages.join("; "))))?;
        self.publish_jar(&info.sha256, &bytes)?;
        let response = bots::submit(
            self.store(),
            config,
            caller.user(),
            &info,
            &body.divisions,
            body.homepage.as_deref(),
            source_url.as_deref(),
            now_ms / 1000,
        )?;
        if body.source == SubmitSource::Upload {
            let _ = self
                .services
                .objects
                .delete(Bucket::State, &upload_key(&uid, &info.sha256));
        }
        self.forget_user(&uid);
        Ok(Response::ok(&response))
    }

    fn retire(&self, request: &Request, caller: &Caller, config: &Config, now_ms: u64) -> Handled {
        let body: BotRequest = request.body_json()?;
        let bot = bots::retire(
            self.store(),
            config,
            caller.user(),
            &body.bot,
            now_ms / 1000,
        )?;
        Ok(Response::ok(&BotStateResponse {
            bot: bot.id,
            state: bot.state.as_str().to_string(),
            owner: bot.owner,
        }))
    }

    // ---- admin ----

    fn import(&self, request: &Request, caller: &Caller, config: &Config, now_ms: u64) -> Handled {
        let body: ImportRequest = request.body_json()?;
        if !is_sha256_hex(&body.sha256) {
            return Err(bad_request("sha256: 64 lower-case hex digits"));
        }
        // A listed version's date (§16.6): no jar.
        if body.date_only {
            return Ok(Response::ok(&bots::set_release_date(self.store(), &body)?));
        }
        let uid = caller.user().uid.clone();
        // A historic version needs its bot (§16.6): checked before its jar is published.
        if body.historic && self.store().get_bot(&body.bot)?.is_none() {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                format!("no bot {}", body.bot),
            ));
        }
        // The server never trusts the client: the jar is validated again from S3 (§16.2).
        let in_jars = self
            .services
            .objects
            .get(Bucket::Jars, &jar_key(&body.sha256), None)
            .map_err(busy)?;
        let (bytes, published) = match in_jars {
            Fetched::Found { bytes, .. } if jar::sha256_hex(&bytes) == body.sha256 => (bytes, true),
            _ => (self.uploaded(&uid, &body.sha256)?, false),
        };
        let info = bots::validate_import(&bytes, config, &body)?;
        if !published {
            self.publish_jar(&info.sha256, &bytes)?;
        }
        let response = bots::import(self.store(), &body, &info, now_ms / 1000)?;
        if !published {
            let _ = self
                .services
                .objects
                .delete(Bucket::State, &upload_key(&uid, &info.sha256));
        }
        Ok(Response::ok(&response))
    }

    fn claim(&self, request: &Request) -> Handled {
        let body: ClaimRequest = request.body_json()?;
        let uid = if parse_uid(&body.github).is_ok() {
            body.github.clone()
        } else {
            match self
                .services
                .github
                .user_id(&body.github)
                .map_err(github_error)?
            {
                Some(id) => rumble_core::ids::uid(id),
                None => {
                    return Err(ApiError::new(
                        ErrorCode::NotFound,
                        format!("no GitHub user {}", body.github),
                    ));
                }
            }
        };
        let bot = bots::claim(self.store(), &body.bot, &uid)?;
        self.forget_user(&uid);
        Ok(Response::ok(&BotStateResponse {
            bot: bot.id,
            state: bot.state.as_str().to_string(),
            owner: bot.owner,
        }))
    }

    fn remove(&self, request: &Request, now_ms: u64) -> Handled {
        let body: RemoveRequest = request.body_json()?;
        let removed = bots::remove(self.store(), &body.bot, &body.reason, now_ms)?;
        println!("{}", removed.log);
        Ok(Response::ok(&BotStateResponse {
            bot: removed.bot.id,
            state: removed.bot.state.as_str().to_string(),
            owner: removed.bot.owner,
        }))
    }

    fn update_user(&self, request: &Request, config: &Config, now_ms: u64) -> Handled {
        let body: UserUpdateRequest = request.body_json()?;
        let user = bots::update_user(self.store(), &body, now_ms)?;
        self.forget_user(&user.uid);
        Ok(Response::ok(&auth::me(config, &user)))
    }

    fn alerts(&self) -> Handled {
        let alerts = self
            .store()
            .list_alerts()?
            .into_iter()
            .map(|a| AlertInfo {
                id: a.sort,
                kind: a.kind,
                text: a.text,
                reference: a.reference,
            })
            .collect();
        Ok(Response::ok(&AlertsResponse { alerts }))
    }

    fn get_config(&self) -> Handled {
        let overrides = match self
            .services
            .objects
            .get(Bucket::State, CONFIG_KEY, None)
            .map_err(busy)?
        {
            Fetched::Found { bytes, .. } => {
                serde_json::from_slice(&bytes).map_err(|e| busy(format!("{CONFIG_KEY}: {e}")))?
            }
            _ => Value::Object(Default::default()),
        };
        Ok(Response::ok(&overrides))
    }

    fn set_config(&self, request: &Request) -> Handled {
        let overrides: Value = request.body_json()?;
        let config = config_from(&overrides).map_err(bad_request)?;
        let bytes = serde_json::to_vec_pretty(&overrides).expect("serializable");
        self.services
            .objects
            .put(Bucket::State, CONFIG_KEY, &bytes, "application/json", None)
            .map_err(busy)?;
        println!("config replaced: {}", String::from_utf8_lossy(&bytes));
        let mut inner = self.lock();
        inner.config = config.clone();
        inner.config_checked = None;
        Ok(Response::ok(&config))
    }
}

#[derive(Clone, Copy)]
enum Limit {
    Work,
    Results,
    Other,
}
