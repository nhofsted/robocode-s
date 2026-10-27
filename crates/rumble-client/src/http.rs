// SPDX-License-Identifier: EPL-2.0

//! The blocking HTTP client of the rumble API (§5): `https://<domain>/api/v1/`.
//!
//! Every request carries `X-Rumble-Client`, and `X-Rumble-Token` when a token is set; every
//! POST carries `x-amz-content-sha256` of the exact body bytes (§5.1: the function URL
//! rejects bodies without it).

use rumble_core::api::{
    AuthTokenRequest, AuthTokenResponse, ErrorBody, ErrorCode, Info, Me, NondeterministicRequest,
    RecordingsDoneRequest, RecordingsDoneResponse, ResultsRequest, ResultsResponse, SubmitRequest,
    SubmitResponse, UploadUrlRequest, UploadUrlResponse, WorkQuery, WorkResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::time::Duration;

/// Hex SHA-256 of `bytes`, lowercase.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `robocode-s/<release> (engine <E>; <os>-<arch>)`.
pub fn client_header(engine: u32) -> String {
    format!(
        "robocode-s/{} (engine {engine}; {}-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// What went wrong with a call. The API variants are the rows of the §5.2 table; `message` is
/// the server's text for humans.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// No answer: DNS, connect, TLS, timeout, broken connection. Retry with backoff.
    Network(String),
    /// 400
    BadRequest(String),
    /// 401: no or unknown token; log in again.
    Unauthorized(String),
    /// 403: banned, not an admin, not the bot's owner.
    Forbidden(String),
    /// 404
    NotFound(String),
    /// 409 `engine_mismatch`: update needed.
    EngineMismatch(String),
    /// 409 `expired`: the batch is past its deadline; drop it.
    Expired(String),
    /// 409 `conflict`
    Conflict(String),
    /// 413
    TooLarge(String),
    /// 426: `message` holds the download page.
    ClientOutdated(String),
    /// 429, with the parsed `Retry-After`.
    RateLimited {
        message: String,
        retry_after: Option<Duration>,
    },
    /// 503 `busy`, with the parsed `Retry-After` (30 s from the server).
    Busy {
        message: String,
        retry_after: Option<Duration>,
    },
    /// Any other status, or a known status with an unknown code.
    Unexpected {
        status: u16,
        message: String,
        retry_after: Option<Duration>,
    },
    /// A 2xx whose body isn't what the endpoint promises.
    Decode(String),
}

impl Error {
    /// The `Retry-After` the server sent, if this error carries one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::RateLimited { retry_after, .. }
            | Error::Busy { retry_after, .. }
            | Error::Unexpected { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Worth trying again later (network trouble, 429, 503, 5xx).
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Network(_) | Error::RateLimited { .. } | Error::Busy { .. } => true,
            Error::Unexpected { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// The §5.2 code, for the API variants.
    pub fn code(&self) -> Option<ErrorCode> {
        Some(match self {
            Error::BadRequest(_) => ErrorCode::BadRequest,
            Error::Unauthorized(_) => ErrorCode::Unauthorized,
            Error::Forbidden(_) => ErrorCode::Forbidden,
            Error::NotFound(_) => ErrorCode::NotFound,
            Error::EngineMismatch(_) => ErrorCode::EngineMismatch,
            Error::Expired(_) => ErrorCode::Expired,
            Error::Conflict(_) => ErrorCode::Conflict,
            Error::TooLarge(_) => ErrorCode::TooLarge,
            Error::ClientOutdated(_) => ErrorCode::ClientOutdated,
            Error::RateLimited { .. } => ErrorCode::RateLimited,
            Error::Busy { .. } => ErrorCode::Busy,
            _ => return None,
        })
    }

    /// The server's message, or the description of the local failure.
    pub fn message(&self) -> &str {
        match self {
            Error::Network(m)
            | Error::BadRequest(m)
            | Error::Unauthorized(m)
            | Error::Forbidden(m)
            | Error::NotFound(m)
            | Error::EngineMismatch(m)
            | Error::Expired(m)
            | Error::Conflict(m)
            | Error::TooLarge(m)
            | Error::ClientOutdated(m)
            | Error::Decode(m) => m,
            Error::RateLimited { message, .. }
            | Error::Busy { message, .. }
            | Error::Unexpected { message, .. } => message,
        }
    }

    fn from_response(status: u16, body: &[u8], retry_after: Option<Duration>) -> Error {
        let parsed: Option<ErrorBody> = serde_json::from_slice(body).ok();
        let message = match &parsed {
            Some(b) if !b.message.is_empty() => b.message.clone(),
            _ => String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned(),
        };
        // The code decides; the status only when the body has none we know.
        let code = parsed.as_ref().and_then(ErrorBody::code).or(match status {
            400 => Some(ErrorCode::BadRequest),
            401 => Some(ErrorCode::Unauthorized),
            403 => Some(ErrorCode::Forbidden),
            404 => Some(ErrorCode::NotFound),
            413 => Some(ErrorCode::TooLarge),
            426 => Some(ErrorCode::ClientOutdated),
            429 => Some(ErrorCode::RateLimited),
            503 => Some(ErrorCode::Busy),
            _ => None,
        });
        match code {
            Some(ErrorCode::BadRequest) => Error::BadRequest(message),
            Some(ErrorCode::Unauthorized) => Error::Unauthorized(message),
            Some(ErrorCode::Forbidden) => Error::Forbidden(message),
            Some(ErrorCode::NotFound) => Error::NotFound(message),
            Some(ErrorCode::EngineMismatch) => Error::EngineMismatch(message),
            Some(ErrorCode::Expired) => Error::Expired(message),
            Some(ErrorCode::Conflict) => Error::Conflict(message),
            Some(ErrorCode::TooLarge) => Error::TooLarge(message),
            Some(ErrorCode::ClientOutdated) => Error::ClientOutdated(message),
            Some(ErrorCode::RateLimited) => Error::RateLimited {
                message,
                retry_after,
            },
            Some(ErrorCode::Busy) => Error::Busy {
                message,
                retry_after,
            },
            None => Error::Unexpected {
                status,
                message,
                retry_after,
            },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Network(m) => write!(f, "network error: {m}"),
            Error::Unexpected {
                status, message, ..
            } => write!(f, "unexpected response {status}: {message}"),
            Error::Decode(m) => write!(f, "unreadable response: {m}"),
            other => match other.code() {
                Some(code) => write!(f, "{}: {}", code.as_str(), other.message()),
                None => write!(f, "{}", other.message()),
            },
        }
    }
}

impl std::error::Error for Error {}

/// The answer of `GET work`.
#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    Batch(WorkResponse),
    /// 204: paused, or nothing for the offered divisions; wait this long (from `Retry-After`).
    Wait(Option<Duration>),
}

/// The reply of a call, before the status is judged.
struct Raw {
    status: u16,
    retry_after: Option<Duration>,
    body: Vec<u8>,
}

/// Parses `Retry-After` given as delta-seconds (what the API sends); HTTP dates are ignored.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

const MAX_BODY: u64 = 16 << 20;

/// The API client. Cheap to clone (the agent shares its connection pool).
#[derive(Clone)]
pub struct Api {
    agent: ureq::Agent,
    base: String,
    token: Option<String>,
    client: String,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "…"))
            .finish()
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // Statuses are judged here, against the §5.2 table.
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into()
}

impl Api {
    /// `base_url` is the API root, e.g. `https://rumble.example/api/v1/` (a trailing slash is
    /// added). `engine` is the engine version of this build, for `X-Rumble-Client`.
    pub fn new(base_url: &str, token: Option<String>, engine: u32) -> Api {
        let mut base = base_url.trim_end_matches('/').to_string();
        base.push('/');
        Api {
            agent: agent(),
            base,
            token,
            client: client_header(engine),
        }
    }

    pub fn with_token(mut self, token: Option<String>) -> Api {
        self.token = token;
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Scheme and host of the base URL, for the server-relative URLs of `GET work`
    /// (`/jars/<sha>.jar`).
    pub fn origin(&self) -> &str {
        let after_scheme = self.base.find("://").map_or(0, |i| i + 3);
        match self.base[after_scheme..].find('/') {
            Some(i) => &self.base[..after_scheme + i],
            None => self.base.trim_end_matches('/'),
        }
    }

    /// An absolute URL as is; a server-relative one against [`Api::origin`].
    pub fn resolve(&self, url: &str) -> String {
        if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            format!("{}/{}", self.origin(), url.trim_start_matches('/'))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path.trim_start_matches('/'))
    }

    fn raw(
        &self,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Raw, Error> {
        let mut response = result.map_err(|e| Error::Network(e.to_string()))?;
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Raw {
            status,
            retry_after,
            body,
        })
    }

    fn get_raw(&self, path: &str) -> Result<Raw, Error> {
        let mut request = self.agent.get(self.url(path));
        request = request.header("X-Rumble-Client", &self.client);
        if let Some(token) = &self.token {
            request = request.header("X-Rumble-Token", token);
        }
        self.raw(request.call())
    }

    fn post_raw(&self, path: &str, body: &[u8]) -> Result<Raw, Error> {
        let mut request = self
            .agent
            .post(self.url(path))
            .header("X-Rumble-Client", &self.client)
            .header("x-amz-content-sha256", sha256_hex(body))
            .content_type("application/json");
        if let Some(token) = &self.token {
            request = request.header("X-Rumble-Token", token);
        }
        self.raw(request.send(body))
    }

    fn judge(raw: Raw) -> Result<Raw, Error> {
        if (200..300).contains(&raw.status) {
            Ok(raw)
        } else {
            Err(Error::from_response(raw.status, &raw.body, raw.retry_after))
        }
    }

    fn decode<T: DeserializeOwned>(raw: Raw) -> Result<T, Error> {
        serde_json::from_slice(&raw.body).map_err(|e| Error::Decode(e.to_string()))
    }

    /// `GET path` (relative to the API root, query included) decoded as `T`.
    pub fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        Self::decode(Self::judge(self.get_raw(path)?)?)
    }

    /// `POST path` with `body` as JSON, the answer decoded as `T`.
    pub fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, Error> {
        let bytes = serde_json::to_vec(body).map_err(|e| Error::Decode(e.to_string()))?;
        Self::decode(Self::judge(self.post_raw(path, &bytes)?)?)
    }

    // ---- public and client endpoints ----

    /// `GET info`
    pub fn info(&self) -> Result<Info, Error> {
        self.get_json("info")
    }

    /// `POST auth/token`: exchanges a GitHub device-flow token for an upload token.
    pub fn auth_token(&self, github_token: &str, label: &str) -> Result<AuthTokenResponse, Error> {
        self.post_json(
            "auth/token",
            &AuthTokenRequest {
                github_token: github_token.to_string(),
                label: label.to_string(),
            },
        )
    }

    /// `GET me`
    pub fn me(&self) -> Result<Me, Error> {
        self.get_json("me")
    }

    /// `POST me`: changes the country.
    pub fn set_country(&self, country: &str) -> Result<Me, Error> {
        self.post_json("me", &json!({ "country": country }))
    }

    /// `GET me/tokens`
    pub fn tokens(&self) -> Result<Value, Error> {
        self.get_json("me/tokens")
    }

    /// `POST me/tokens/revoke`; `"current"` revokes the caller's own token.
    pub fn revoke_token(&self, id: &str) -> Result<Value, Error> {
        self.post_json("me/tokens/revoke", &json!({ "id": id }))
    }

    /// `GET me/bots`
    pub fn my_bots(&self) -> Result<Value, Error> {
        self.get_json("me/bots")
    }

    /// `GET work`: a batch, or the wait of a 204.
    pub fn work(&self, query: &WorkQuery) -> Result<Work, Error> {
        let raw = self.get_raw(&format!("work?{}", query.to_query()))?;
        if raw.status == 204 {
            return Ok(Work::Wait(raw.retry_after));
        }
        Ok(Work::Batch(Self::decode(Self::judge(raw)?)?))
    }

    /// `POST results`
    pub fn results(&self, request: &ResultsRequest) -> Result<ResultsResponse, Error> {
        self.post_json("results", request)
    }

    /// `POST recordings/done`
    pub fn recordings_done(
        &self,
        request: &RecordingsDoneRequest,
    ) -> Result<RecordingsDoneResponse, Error> {
        self.post_json("recordings/done", request)
    }

    /// `POST bots/upload-url`
    pub fn upload_url(&self, request: &UploadUrlRequest) -> Result<UploadUrlResponse, Error> {
        self.post_json("bots/upload-url", request)
    }

    /// `POST bots/submit`
    pub fn submit(&self, request: &SubmitRequest) -> Result<SubmitResponse, Error> {
        self.post_json("bots/submit", request)
    }

    /// `POST bots/retire`
    pub fn retire(&self, bot: &str) -> Result<Value, Error> {
        self.post_json("bots/retire", &json!({ "bot": bot }))
    }

    // ---- admin endpoints (§5.4); bodies the importer and tools build themselves ----

    /// `POST admin/jars/upload-url`
    pub fn admin_upload_url(&self, request: &UploadUrlRequest) -> Result<UploadUrlResponse, Error> {
        self.post_json("admin/jars/upload-url", request)
    }

    /// `POST admin/bots/import` (§16)
    pub fn admin_import(&self, body: &Value) -> Result<Value, Error> {
        self.post_json("admin/bots/import", body)
    }

    /// `POST admin/bots/claim`
    pub fn admin_claim(&self, bot: &str, github: &str) -> Result<Value, Error> {
        self.post_json("admin/bots/claim", &json!({ "bot": bot, "github": github }))
    }

    /// `POST admin/bots/remove`
    pub fn admin_remove(&self, bot: &str, reason: &str) -> Result<Value, Error> {
        self.post_json(
            "admin/bots/remove",
            &json!({ "bot": bot, "reason": reason }),
        )
    }

    /// `POST admin/bots/nd`
    pub fn admin_nondeterministic(&self, body: &NondeterministicRequest) -> Result<Value, Error> {
        self.post_json("admin/bots/nd", body)
    }

    /// `POST admin/users/update`; `body` has `uid` and any of `rep`, `state`, `admin`.
    pub fn admin_users_update(&self, body: &Value) -> Result<Value, Error> {
        self.post_json("admin/users/update", body)
    }

    /// `GET admin/alerts`
    pub fn admin_alerts(&self) -> Result<Value, Error> {
        self.get_json("admin/alerts")
    }

    /// `GET admin/config`
    pub fn admin_config(&self) -> Result<Value, Error> {
        self.get_json("admin/config")
    }

    /// `POST admin/config`: replaces `state/config.json`.
    pub fn admin_set_config(&self, config: &Value) -> Result<Value, Error> {
        self.post_json("admin/config", config)
    }

    // ---- bytes ----

    /// `GET` of a jar (or any file) at an absolute or server-relative URL, at most `limit`
    /// bytes. No token is sent: the files are public.
    pub fn download(&self, url: &str, limit: u64) -> Result<Vec<u8>, Error> {
        let response = self
            .agent
            .get(self.resolve(url))
            .header("X-Rumble-Client", &self.client)
            .call();
        let mut response = response.map_err(|e| Error::Network(e.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after);
            let body = response
                .body_mut()
                .with_config()
                .limit(4096)
                .read_to_vec()
                .unwrap_or_default();
            return Err(Error::from_response(status, &body, retry_after));
        }
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| Error::Network(e.to_string()))
    }

    /// `PUT` to a presigned URL with exactly the given headers (the signed ones). `Content-Length`
    /// is set by the transport from `bytes`; a given one that disagrees is an error.
    pub fn put_presigned<'a>(
        &self,
        url: &str,
        headers: impl IntoIterator<Item = (&'a str, &'a str)>,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let mut request = self.agent.put(url);
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("content-length") {
                if value.trim().parse::<usize>().ok() != Some(bytes.len()) {
                    return Err(Error::BadRequest(format!(
                        "Content-Length {value} but {} bytes to send",
                        bytes.len()
                    )));
                }
                continue;
            }
            request = request.header(name, value);
        }
        let raw = self.raw(request.send(bytes))?;
        if (200..300).contains(&raw.status) {
            Ok(())
        } else {
            // S3 answers in XML, not our JSON.
            Err(Error::Unexpected {
                status: raw.status,
                message: String::from_utf8_lossy(&raw.body[..raw.body.len().min(300)]).into_owned(),
                retry_after: raw.retry_after,
            })
        }
    }
}

impl From<Error> for io::Error {
    fn from(e: Error) -> io::Error {
        io::Error::other(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parses_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after(" 7 "), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("1.5"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("-3"), None);
    }

    #[test]
    fn sha256_of_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn origin_and_resolve() {
        let api = Api::new("https://r.example/api/v1", None, 1);
        assert_eq!(api.base_url(), "https://r.example/api/v1/");
        assert_eq!(api.origin(), "https://r.example");
        assert_eq!(api.resolve("/jars/a.jar"), "https://r.example/jars/a.jar");
        assert_eq!(api.resolve("http://x/y"), "http://x/y");
        let local = Api::new("http://127.0.0.1:8080/api/v1/", None, 1);
        assert_eq!(local.origin(), "http://127.0.0.1:8080");
    }

    #[test]
    fn error_mapping() {
        let body = br#"{"error":"conflict","message":"slot filled"}"#;
        assert_eq!(
            Error::from_response(409, body, None),
            Error::Conflict("slot filled".into())
        );
        // Unknown code, known status.
        let e = Error::from_response(429, b"slow down", Some(Duration::from_secs(3)));
        assert_eq!(e.retry_after(), Some(Duration::from_secs(3)));
        assert_eq!(e.code(), Some(ErrorCode::RateLimited));
        assert!(matches!(
            Error::from_response(418, b"", None),
            Error::Unexpected { status: 418, .. }
        ));
        assert!(Error::from_response(502, b"", None).is_retryable());
        assert!(!Error::from_response(401, b"", None).is_retryable());
    }
}
