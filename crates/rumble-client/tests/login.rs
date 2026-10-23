// SPDX-License-Identifier: EPL-2.0
//! The device flow against a fake GitHub and a fake rumble server (one server, two path
//! prefixes).

mod support;

use rumble_client::http::{Api, Error};
use rumble_client::login::{DeviceCode, DeviceFlow, LoginError};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Req, Resp, Server};

fn me_json() -> Value {
    json!({"uid": "g1", "login": "nick", "name": "Nick", "country": "be", "reputation": 3,
           "tier": "new", "verifyRate": 0.5, "battles": 2, "verifiedMatches": 0,
           "mismatches": 0, "admin": false, "since": "2026-10-24T00:00:00Z"})
}

/// `polls` are the answers to successive polls of the access-token endpoint (the last one
/// repeats).
fn fake(polls: Vec<Value>, expires_in: u64) -> Server {
    let n = AtomicUsize::new(0);
    Server::start(move |req: &Req| match req.path.as_str() {
        "/api/v1/info" => Resp::json(
            200,
            &json!({"api": 1, "engineVersion": 1, "minClient": "0.1.0", "paused": false,
                    "githubClientId": "Ov23test", "deterministicRules": 1, "divisions": [],
                    "downloads": "https://x"}),
        ),
        "/login/device/code" => Resp::json(
            200,
            &json!({"device_code": "dc-123", "user_code": "ABCD-1234",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": expires_in, "interval": 5}),
        ),
        "/login/oauth/access_token" => {
            let i = n.fetch_add(1, Ordering::SeqCst);
            Resp::json(200, &polls[i.min(polls.len() - 1)])
        }
        "/api/v1/auth/token" => {
            if req.body_text().contains("gho_good") {
                Resp::json(200, &json!({"token": "rsu_new", "user": me_json()}))
            } else {
                Resp::json(
                    401,
                    &json!({"error": "unauthorized", "message": "GitHub said no"}),
                )
            }
        }
        other => panic!("unexpected {other}"),
    })
}

fn flow(server: &Server, sleeps: &Arc<Mutex<Vec<Duration>>>) -> DeviceFlow {
    let sleeps = sleeps.clone();
    DeviceFlow {
        github: server.url(""),
        sleep: Box::new(move |d| sleeps.lock().unwrap().push(d)),
        cancelled: Box::new(|| false),
    }
}

fn secs(sleeps: &Arc<Mutex<Vec<Duration>>>) -> Vec<u64> {
    sleeps.lock().unwrap().iter().map(|d| d.as_secs()).collect()
}

fn form(req: &Req) -> String {
    req.body_text()
}

#[test]
fn full_flow_with_pending_and_slow_down() {
    let server = fake(
        vec![
            json!({"error": "authorization_pending"}),
            json!({"error": "slow_down"}),
            json!({"error": "authorization_pending"}),
            json!({"access_token": "gho_good", "token_type": "bearer"}),
        ],
        900,
    );
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let mut flow = flow(&server, &sleeps);
    let api = Api::new(&server.url("/api/v1"), None, 1);
    let mut shown: Vec<DeviceCode> = Vec::new();
    let response = flow
        .login(&api, "nick-desktop", &mut |c| shown.push(c.clone()))
        .unwrap();

    assert_eq!(response.token, "rsu_new");
    assert_eq!(response.user.login, "nick");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].user_code, "ABCD-1234");
    assert_eq!(shown[0].verification_uri, "https://github.com/login/device");
    // 5 s, then slow_down adds 5 s for every later poll.
    assert_eq!(secs(&sleeps), [5, 5, 10, 10]);

    let requests = server.requests();
    let code = requests
        .iter()
        .find(|r| r.path == "/login/device/code")
        .unwrap();
    assert_eq!(form(code), "client_id=Ov23test&scope=");
    assert_eq!(code.header("accept"), Some("application/json"));
    let polls: Vec<&Req> = requests
        .iter()
        .filter(|r| r.path == "/login/oauth/access_token")
        .collect();
    assert_eq!(polls.len(), 4);
    assert_eq!(
        form(polls[0]),
        "client_id=Ov23test&device_code=dc-123&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
    );
    // The upload token request carries the GitHub token and the label.
    let exchange = requests.last().unwrap();
    assert_eq!(exchange.path, "/api/v1/auth/token");
    let body: Value = serde_json::from_slice(&exchange.body).unwrap();
    assert_eq!(
        body,
        json!({"githubToken": "gho_good", "label": "nick-desktop"})
    );
    assert!(exchange.header("x-amz-content-sha256").is_some());
}

#[test]
fn slow_down_with_an_interval_in_the_answer() {
    let server = fake(
        vec![
            json!({"error": "slow_down", "interval": 20}),
            json!({"access_token": "gho_good"}),
        ],
        900,
    );
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let mut flow = flow(&server, &sleeps);
    let api = Api::new(&server.url("/api/v1"), None, 1);
    flow.login(&api, "l", &mut |_| {}).unwrap();
    assert_eq!(secs(&sleeps), [5, 20]);
}

#[test]
fn the_code_expires() {
    // GitHub says so.
    let server = fake(
        vec![
            json!({"error": "authorization_pending"}),
            json!({"error": "expired_token"}),
        ],
        900,
    );
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let mut f = flow(&server, &sleeps);
    let api = Api::new(&server.url("/api/v1"), None, 1);
    assert!(matches!(
        f.login(&api, "l", &mut |_| {}),
        Err(LoginError::Expired)
    ));
    assert_eq!(secs(&sleeps), [5, 5]);

    // We notice by ourselves when `expires_in` has passed: 12 s allows polls after 5 and 10 s,
    // and the one after 15 s is the last.
    let server = fake(vec![json!({"error": "authorization_pending"})], 12);
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let mut f = flow(&server, &sleeps);
    let api = Api::new(&server.url("/api/v1"), None, 1);
    assert!(matches!(
        f.login(&api, "l", &mut |_| {}),
        Err(LoginError::Expired)
    ));
    assert_eq!(secs(&sleeps), [5, 5, 5]);
}

#[test]
fn denied_and_other_github_errors() {
    let server = fake(vec![json!({"error": "access_denied"})], 900);
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let api = Api::new(&server.url("/api/v1"), None, 1);
    assert!(matches!(
        flow(&server, &sleeps).login(&api, "l", &mut |_| {}),
        Err(LoginError::Denied)
    ));

    let server = fake(
        vec![
            json!({"error": "device_flow_disabled", "error_description": "Device Flow must be explicitly enabled"}),
        ],
        900,
    );
    let api = Api::new(&server.url("/api/v1"), None, 1);
    match flow(&server, &sleeps).login(&api, "l", &mut |_| {}) {
        Err(LoginError::GitHub(m)) => assert!(m.contains("device_flow_disabled"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cancel_stops_the_poll() {
    let server = fake(vec![json!({"error": "authorization_pending"})], 900);
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let mut f = flow(&server, &sleeps);
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = polls.clone();
    f.cancelled = Box::new(move || counter.fetch_add(1, Ordering::SeqCst) >= 4);
    let api = Api::new(&server.url("/api/v1"), None, 1);
    assert!(matches!(
        f.login(&api, "l", &mut |_| {}),
        Err(LoginError::Cancelled)
    ));
}

#[test]
fn the_server_refusing_the_github_token_is_reported() {
    let server = fake(vec![json!({"access_token": "gho_other_app"})], 900);
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let api = Api::new(&server.url("/api/v1"), None, 1);
    match flow(&server, &sleeps).login(&api, "l", &mut |_| {}) {
        Err(LoginError::Server(Error::Unauthorized(m))) => assert_eq!(m, "GitHub said no"),
        other => panic!("{other:?}"),
    }
}
