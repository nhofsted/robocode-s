// SPDX-License-Identifier: EPL-2.0

//! GitHub device-flow login (§7.2). The client never holds a secret: it asks GitHub for a
//! code, shows it (the `show` callback: the CLI prints, the GUI draws), polls until the user
//! approved it, and exchanges the GitHub token for an upload token with `POST auth/token`.

use crate::http::{self, Api};
use rumble_core::api::AuthTokenResponse;
use serde::Deserialize;
use std::fmt;
use std::time::Duration;

pub const GITHUB: &str = "https://github.com";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Transient network errors tolerated in a row while polling.
const POLL_NETWORK_RETRIES: u32 = 3;

/// What GitHub returns for the code request; show `user_code` and `verification_uri`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug)]
pub enum LoginError {
    /// GitHub couldn't be reached.
    Network(String),
    /// GitHub refused: bad client id, device flow not enabled for the app, ...
    GitHub(String),
    /// The code expired before the user approved it.
    Expired,
    /// The user denied the request.
    Denied,
    /// `cancelled` returned true.
    Cancelled,
    /// The rumble server refused the exchange.
    Server(http::Error),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginError::Network(m) => write!(f, "can't reach GitHub: {m}"),
            LoginError::GitHub(m) => write!(f, "GitHub: {m}"),
            LoginError::Expired => write!(f, "the code expired; start the login again"),
            LoginError::Denied => write!(f, "the login was denied on GitHub"),
            LoginError::Cancelled => write!(f, "login cancelled"),
            LoginError::Server(e) => write!(f, "rumble server: {e}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// Where GitHub is, how to wait, and how to cancel.
pub struct DeviceFlow {
    /// GitHub's base URL; tests point it at a fake.
    pub github: String,
    /// Waits between polls. Real sleep by default; tests record instead.
    pub sleep: Box<dyn FnMut(Duration)>,
    /// Checked before every poll; true ends the flow with `Cancelled`.
    pub cancelled: Box<dyn Fn() -> bool>,
}

impl Default for DeviceFlow {
    fn default() -> Self {
        DeviceFlow {
            github: GITHUB.to_string(),
            sleep: Box::new(std::thread::sleep),
            cancelled: Box::new(|| false),
        }
    }
}

fn form_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn post_form(
    agent: &ureq::Agent,
    url: &str,
    fields: &[(&str, &str)],
) -> Result<serde_json::Value, LoginError> {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let mut response = agent
        .post(url)
        .header("Accept", "application/json")
        .content_type("application/x-www-form-urlencoded")
        .send(body.as_bytes())
        .map_err(|e| LoginError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_vec()
        .map_err(|e| LoginError::Network(e.to_string()))?;
    // GitHub reports flow errors as 200 with an `error` field, but be lenient with others.
    serde_json::from_slice(&text).map_err(|_| {
        LoginError::GitHub(format!(
            "unexpected answer ({status}): {}",
            String::from_utf8_lossy(&text[..text.len().min(200)])
        ))
    })
}

fn error_text(value: &serde_json::Value) -> String {
    let code = value["error"].as_str().unwrap_or("unknown error");
    match value["error_description"].as_str() {
        Some(d) if !d.is_empty() => format!("{code}: {d}"),
        _ => code.to_string(),
    }
}

impl DeviceFlow {
    /// Step 1: `POST <github>/login/device/code`.
    pub fn request_code(&self, client_id: &str) -> Result<DeviceCode, LoginError> {
        let agent = http_agent();
        let value = post_form(
            &agent,
            &format!("{}/login/device/code", self.github.trim_end_matches('/')),
            &[("client_id", client_id), ("scope", "")],
        )?;
        if value.get("error").is_some() {
            return Err(LoginError::GitHub(error_text(&value)));
        }
        serde_json::from_value(value).map_err(|e| LoginError::GitHub(e.to_string()))
    }

    /// Step 2: polls `POST <github>/login/oauth/access_token` until the user approved; returns
    /// the GitHub token.
    pub fn poll(&mut self, client_id: &str, code: &DeviceCode) -> Result<String, LoginError> {
        let agent = http_agent();
        let url = format!(
            "{}/login/oauth/access_token",
            self.github.trim_end_matches('/')
        );
        let mut interval = Duration::from_secs(code.interval.max(1));
        let limit = Duration::from_secs(code.expires_in);
        let mut waited = Duration::ZERO;
        let mut network_failures = 0;
        loop {
            if (self.cancelled)() {
                return Err(LoginError::Cancelled);
            }
            (self.sleep)(interval);
            waited += interval;
            if (self.cancelled)() {
                return Err(LoginError::Cancelled);
            }
            let value = match post_form(
                &agent,
                &url,
                &[
                    ("client_id", client_id),
                    ("device_code", &code.device_code),
                    ("grant_type", GRANT_TYPE),
                ],
            ) {
                Ok(v) => {
                    network_failures = 0;
                    v
                }
                Err(LoginError::Network(m)) => {
                    network_failures += 1;
                    if network_failures >= POLL_NETWORK_RETRIES {
                        return Err(LoginError::Network(m));
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            if let Some(token) = value["access_token"].as_str() {
                return Ok(token.to_string());
            }
            match value["error"].as_str() {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    // GitHub's answer carries the new interval; else add the 5 s of the spec.
                    interval = match value["interval"].as_u64() {
                        Some(secs) => Duration::from_secs(secs).max(interval),
                        None => interval + Duration::from_secs(5),
                    };
                }
                Some("expired_token") => return Err(LoginError::Expired),
                Some("access_denied") => return Err(LoginError::Denied),
                _ => return Err(LoginError::GitHub(error_text(&value))),
            }
            if waited >= limit {
                return Err(LoginError::Expired);
            }
        }
    }

    /// The whole login: `GET info` for the client id, the code, `show`, the poll, then
    /// `POST auth/token` with `label` (the machine's name). The `api` needs no token.
    pub fn login(
        &mut self,
        api: &Api,
        label: &str,
        show: &mut dyn FnMut(&DeviceCode),
    ) -> Result<AuthTokenResponse, LoginError> {
        let info = api.info().map_err(LoginError::Server)?;
        let code = self.request_code(&info.github_client_id)?;
        show(&code);
        let github_token = self.poll(&info.github_client_id, &code)?;
        api.auth_token(&github_token, label)
            .map_err(LoginError::Server)
    }
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding() {
        assert_eq!(
            form_encode(GRANT_TYPE),
            "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
        );
        assert_eq!(form_encode("a b&c"), "a%20b%26c");
    }
}
