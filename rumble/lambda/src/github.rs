// SPDX-License-Identifier: EPL-2.0

//! GitHub (spec §7): the web flow's code exchange, the profile, the application token API
//! that proves a device-flow token was issued to our OAuth app, deleting tokens we no longer
//! need, and looking up a login (for claims). Behind [`Github`] so the router's tests use
//! [`FakeGithub`].

use crate::block_on;
use rumble_core::auth::GithubProfile;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GithubError {
    /// GitHub refused the code or token (401 for our callers).
    Rejected(String),
    /// GitHub couldn't be reached or answered oddly (503).
    Unavailable(String),
}

impl std::fmt::Display for GithubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GithubError::Rejected(why) => write!(f, "GitHub refused: {why}"),
            GithubError::Unavailable(why) => write!(f, "GitHub unavailable: {why}"),
        }
    }
}

pub type GithubResult<T> = Result<T, GithubError>;

pub trait Github: Send + Sync {
    /// `POST https://github.com/login/oauth/access_token` with our id, secret and `code`.
    fn exchange_code(&self, code: &str, redirect_uri: &str) -> GithubResult<String>;
    /// `GET https://api.github.com/user`.
    fn user(&self, token: &str) -> GithubResult<GithubProfile>;
    /// `POST /applications/{client_id}/token`: the user id if the token is valid and was
    /// issued to our app.
    fn check_token(&self, token: &str) -> GithubResult<u64>;
    /// `DELETE /applications/{client_id}/token`.
    fn delete_token(&self, token: &str) -> GithubResult<()>;
    /// `GET /users/{login}`: the numeric id, `None` if there is no such user.
    fn user_id(&self, login: &str) -> GithubResult<Option<u64>>;
}

impl<G: Github + ?Sized> Github for std::sync::Arc<G> {
    fn exchange_code(&self, code: &str, redirect_uri: &str) -> GithubResult<String> {
        (**self).exchange_code(code, redirect_uri)
    }
    fn user(&self, token: &str) -> GithubResult<GithubProfile> {
        (**self).user(token)
    }
    fn check_token(&self, token: &str) -> GithubResult<u64> {
        (**self).check_token(token)
    }
    fn delete_token(&self, token: &str) -> GithubResult<()> {
        (**self).delete_token(token)
    }
    fn user_id(&self, login: &str) -> GithubResult<Option<u64>> {
        (**self).user_id(login)
    }
}

/// Parses GitHub's `created_at` (`2011-01-25T18:44:36Z`) to unix seconds.
pub fn parse_time(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { text.get(range)?.parse().ok() };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

#[derive(Deserialize)]
struct UserJson {
    id: u64,
    login: String,
    name: Option<String>,
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct AppTokenJson {
    user: Option<UserIdJson>,
}

#[derive(Deserialize)]
struct UserIdJson {
    id: u64,
}

#[derive(Deserialize)]
struct AccessTokenJson {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The real GitHub, over `reqwest` (rustls).
pub struct HttpGithub {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    api: String,
    web: String,
}

impl HttpGithub {
    pub fn new(client_id: &str, client_secret: &str) -> HttpGithub {
        HttpGithub::with_hosts(
            client_id,
            client_secret,
            "https://api.github.com",
            "https://github.com",
        )
    }

    /// With other base URLs (tests).
    pub fn with_hosts(client_id: &str, client_secret: &str, api: &str, web: &str) -> HttpGithub {
        let client = reqwest::Client::builder()
            .user_agent("robocode-s-rumble")
            .timeout(Duration::from_secs(8))
            .build()
            .expect("an HTTP client");
        HttpGithub {
            client,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            api: api.trim_end_matches('/').to_string(),
            web: web.trim_end_matches('/').to_string(),
        }
    }

    fn unavailable(e: reqwest::Error) -> GithubError {
        GithubError::Unavailable(e.to_string())
    }

    fn api_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    fn app_token(&self, method: reqwest::Method, token: &str) -> reqwest::RequestBuilder {
        self.api_request(method, &format!("/applications/{}/token", self.client_id))
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .json(&serde_json::json!({ "access_token": token }))
    }
}

async fn status_check(response: reqwest::Response) -> GithubResult<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else if status.as_u16() == 401 || status.as_u16() == 404 || status.as_u16() == 422 {
        Err(GithubError::Rejected(format!("HTTP {status}")))
    } else {
        Err(GithubError::Unavailable(format!("HTTP {status}")))
    }
}

impl Github for HttpGithub {
    fn exchange_code(&self, code: &str, redirect_uri: &str) -> GithubResult<String> {
        block_on(async {
            let response = self
                .client
                .post(format!("{}/login/oauth/access_token", self.web))
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", self.client_id.as_str()),
                    ("client_secret", self.client_secret.as_str()),
                    ("code", code),
                    ("redirect_uri", redirect_uri),
                ])
                .send()
                .await
                .map_err(Self::unavailable)?;
            let body: AccessTokenJson = status_check(response)
                .await?
                .json()
                .await
                .map_err(Self::unavailable)?;
            match (body.access_token, body.error) {
                (Some(token), None) => Ok(token),
                (_, error) => Err(GithubError::Rejected(
                    body.error_description
                        .or(error)
                        .unwrap_or_else(|| "no token".to_string()),
                )),
            }
        })
    }

    fn user(&self, token: &str) -> GithubResult<GithubProfile> {
        block_on(async {
            let response = self
                .api_request(reqwest::Method::GET, "/user")
                .bearer_auth(token)
                .send()
                .await
                .map_err(Self::unavailable)?;
            let user: UserJson = status_check(response)
                .await?
                .json()
                .await
                .map_err(Self::unavailable)?;
            Ok(GithubProfile {
                id: user.id,
                login: user.login,
                name: user.name.filter(|n| !n.is_empty()),
                created_at: user.created_at.as_deref().and_then(parse_time),
            })
        })
    }

    fn check_token(&self, token: &str) -> GithubResult<u64> {
        block_on(async {
            let response = self
                .app_token(reqwest::Method::POST, token)
                .send()
                .await
                .map_err(Self::unavailable)?;
            let body: AppTokenJson = status_check(response)
                .await?
                .json()
                .await
                .map_err(Self::unavailable)?;
            body.user
                .map(|u| u.id)
                .ok_or_else(|| GithubError::Rejected("the token has no user".into()))
        })
    }

    fn delete_token(&self, token: &str) -> GithubResult<()> {
        block_on(async {
            let response = self
                .app_token(reqwest::Method::DELETE, token)
                .send()
                .await
                .map_err(Self::unavailable)?;
            status_check(response).await.map(|_| ())
        })
    }

    fn user_id(&self, login: &str) -> GithubResult<Option<u64>> {
        if login.is_empty()
            || !login
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Ok(None);
        }
        block_on(async {
            let response = self
                .api_request(reqwest::Method::GET, &format!("/users/{login}"))
                .basic_auth(&self.client_id, Some(&self.client_secret))
                .send()
                .await
                .map_err(Self::unavailable)?;
            if response.status().as_u16() == 404 {
                return Ok(None);
            }
            let user: UserJson = status_check(response)
                .await?
                .json()
                .await
                .map_err(Self::unavailable)?;
            Ok(Some(user.id))
        })
    }
}

/// An in-memory GitHub for tests: codes and tokens map to profiles; tokens of "other apps"
/// pass `user` but not `check_token`.
#[derive(Default)]
pub struct FakeGithub {
    pub state: Mutex<FakeState>,
}

#[derive(Default)]
pub struct FakeState {
    /// code → token
    pub codes: HashMap<String, String>,
    /// token → profile
    pub tokens: HashMap<String, GithubProfile>,
    /// Tokens issued to another OAuth app.
    pub foreign: Vec<String>,
    /// Tokens deleted through the application API.
    pub deleted: Vec<String>,
}

impl FakeGithub {
    pub fn new() -> FakeGithub {
        FakeGithub::default()
    }

    pub fn add_token(&self, token: &str, profile: GithubProfile) {
        self.state
            .lock()
            .unwrap()
            .tokens
            .insert(token.to_string(), profile);
    }

    pub fn add_code(&self, code: &str, token: &str, profile: GithubProfile) {
        self.add_token(token, profile);
        self.state
            .lock()
            .unwrap()
            .codes
            .insert(code.to_string(), token.to_string());
    }

    pub fn deleted(&self) -> Vec<String> {
        self.state.lock().unwrap().deleted.clone()
    }
}

impl Github for FakeGithub {
    fn exchange_code(&self, code: &str, _redirect_uri: &str) -> GithubResult<String> {
        self.state
            .lock()
            .unwrap()
            .codes
            .get(code)
            .cloned()
            .ok_or_else(|| GithubError::Rejected("bad code".into()))
    }

    fn user(&self, token: &str) -> GithubResult<GithubProfile> {
        self.state
            .lock()
            .unwrap()
            .tokens
            .get(token)
            .cloned()
            .ok_or_else(|| GithubError::Rejected("bad token".into()))
    }

    fn check_token(&self, token: &str) -> GithubResult<u64> {
        let state = self.state.lock().unwrap();
        if state.foreign.iter().any(|t| t == token) {
            return Err(GithubError::Rejected("not our app".into()));
        }
        state
            .tokens
            .get(token)
            .map(|p| p.id)
            .ok_or_else(|| GithubError::Rejected("bad token".into()))
    }

    fn delete_token(&self, token: &str) -> GithubResult<()> {
        let mut state = self.state.lock().unwrap();
        state.deleted.push(token.to_string());
        Ok(())
    }

    fn user_id(&self, login: &str) -> GithubResult<Option<u64>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .tokens
            .values()
            .find(|p| p.login == login)
            .map(|p| p.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-10-24T12:00:00Z"), Some(1_792_843_200));
        assert_eq!(parse_time("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(parse_time("garbage"), None);
    }
}
