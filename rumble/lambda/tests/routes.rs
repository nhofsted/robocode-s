// SPDX-License-Identifier: EPL-2.0

//! The `api` router over `MemStore`, in-memory S3 and a fake GitHub (spec §5.4, §7).

mod common;

use common::*;
use rumble_core::api::{
    BattleKind, BattleStatus, RecordingsDoneRequest, ResultsResponse, WorkResponse,
};
use rumble_core::ids::Division;
use rumble_core::store::{Run, Store, VERIFY_TTL, VerifyItem, VerifyKind, VersionState};
use rumble_core::token;
use rumble_lambda::http::{OAUTH_COOKIE, Request, SESSION_COOKIE};
use rumble_lambda::s3::{Bucket, IMMUTABLE, Objects};
use serde_json::{Value, json};

/// Three rounds per 1v1 battle keep the result bodies small.
fn small() -> Value {
    json!({"divisions": {"1v1": {"rounds": 3}}})
}

fn work_harness() -> Harness {
    let h = Harness::new(Some(small()));
    h.versions(1..=3);
    h.put_plan(&plan_1v1(1));
    h
}

fn work(h: &Harness, token: &str, max: u32) -> WorkResponse {
    let response = h.get(
        &format!("work?engine=1&max={max}&divisions=1v1&slots=2"),
        token,
    );
    assert_eq!(response.status, 200, "{:?}", response.value());
    serde_json::from_slice(&response.body).unwrap()
}

fn cookie_of(response: &rumble_lambda::http::Response, name: &str) -> Option<String> {
    response
        .headers
        .iter()
        .filter(|(n, _)| n == "set-cookie")
        .filter_map(|(_, v)| v.strip_prefix(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap().to_string())
        .next()
}

#[test]
fn info_is_public() {
    let h = Harness::new(None);
    let response = h.send(Request::new("GET", "/api/v1/info"));
    assert_eq!(response.status, 200);
    let info = response.value();
    assert_eq!(info["api"], 1);
    assert_eq!(info["engineVersion"], rumble_lambda::ENGINE_VERSION);
    assert_eq!(info["githubClientId"], "Ov23test");
    assert_eq!(info["downloads"], format!("{SITE}/download"));
    assert_eq!(info["divisions"][1]["robots"], 10);
    assert_eq!(response.get("content-type"), Some("application/json"));
    assert!(response.get("access-control-allow-origin").is_none());
}

#[test]
fn transport_errors() {
    let h = Harness::new(None);
    let token = h.user(1, 0);
    assert_eq!(h.get("nothing", &token).status, 404);
    assert_eq!(h.send(Request::new("GET", "/elsewhere")).status, 404);
    // 401: no token, an unknown token.
    let none = h.send(Request::new("GET", "/api/v1/me"));
    assert_eq!(
        (none.status, none.value()["error"].as_str()),
        (401, Some("unauthorized"))
    );
    assert_eq!(
        h.get("me", "rsu_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .status,
        401
    );
    // 426: an old client.
    let old = h.send(Request::new("GET", "/api/v1/info").header(
        "X-Rumble-Client",
        "robocode-s/0.1.3 (engine 1; linux-x86_64)",
    ));
    assert_eq!(old.status, 426);
    assert_eq!(old.value()["error"], "client_outdated");
    assert!(
        old.value()["message"]
            .as_str()
            .unwrap()
            .contains("/download")
    );
    // 413: a body over 1 MiB.
    let mut big = Request::new("POST", "/api/v1/results").header("X-Rumble-Token", &token);
    big.body = vec![b' '; (1 << 20) + 1];
    assert_eq!(h.send(big).status, 413);
    // 400: malformed JSON.
    let mut bad = Request::new("POST", "/api/v1/me").header("X-Rumble-Token", &token);
    bad.body = b"{".to_vec();
    assert_eq!(h.send(bad).value()["error"], "bad_request");
}

#[test]
fn rate_limits() {
    let h = work_harness();
    let token = h.user(1, 0);
    work(&h, &token, 2);
    // Work: one request per 10 s per token.
    let again = h.get("work?engine=1&max=2", &token);
    assert_eq!(again.status, 429);
    assert_eq!(again.get("retry-after"), Some("10"));
    h.advance(10_000);
    work(&h, &token, 2);
    // Other calls: 2/s with a burst of 10.
    let statuses: Vec<u16> = (0..12).map(|_| h.get("me", &token).status).collect();
    assert_eq!(&statuses[..10], &[200; 10]);
    assert_eq!(statuses[11], 429);
    // Admins are exempt (the importer).
    let admin = h.user(ADMIN_ID, 0);
    let statuses: Vec<u16> = (0..30).map(|_| h.get("me", &admin).status).collect();
    assert_eq!(statuses, [200; 30]);
    // auth/* per viewer IP: 1/s, burst 20.
    let login = || {
        h.send(
            Request::new("GET", "/api/v1/auth/login")
                .header("CloudFront-Viewer-Address", "198.51.100.7:4000"),
        )
        .status
    };
    let statuses: Vec<u16> = (0..21).map(|_| login()).collect();
    assert_eq!(&statuses[..20], &[302; 20]);
    assert_eq!(statuses[20], 429);
}

#[test]
fn work_from_the_plan() {
    let h = Harness::new(Some(small()));
    let token = h.user(1, 0);
    // No plan yet: 204 with Retry-After.
    let none = h.get("work?engine=1&max=5", &token);
    assert_eq!((none.status, none.get("retry-after")), (204, Some("60")));
    h.versions(1..=3);
    h.put_plan(&plan_1v1(1));
    // The plan is checked again only after 60 s.
    h.advance(10_000);
    assert_eq!(h.get("work?engine=1&max=5", &token).status, 204);
    h.advance(60_000);
    let batch = work(&h, &token, 5);
    assert_eq!(batch.battles.len(), 5);
    assert_eq!(batch.engine_version, 1);
    assert!(
        batch
            .battles
            .iter()
            .all(|b| b.kind == BattleKind::Normal && b.rules.rounds == 3)
    );
    let payload = token::verify(&batch.batch, &keys(), h.now()).unwrap();
    assert_eq!(payload.uid, "g1");
    // Engine mismatch.
    h.advance(10_000);
    let other = h.get("work?engine=2&max=5", &token);
    assert_eq!(
        (other.status, other.value()["error"].as_str()),
        (409, Some("engine_mismatch"))
    );
    h.advance(10_000);
    assert_eq!(
        h.get("work?engine=1", &token).status,
        400,
        "max is required"
    );
}

#[test]
fn paused_rumble_gives_204() {
    let h = Harness::new(Some(json!({"paused": true})));
    h.versions(1..=3);
    h.put_plan(&plan_1v1(rumble_lambda::ENGINE_VERSION));
    let token = h.user(1, 0);
    let response = h.get("work?engine=1&max=5", &token);
    assert_eq!(
        (response.status, response.get("retry-after")),
        (204, Some("300"))
    );
}

#[test]
fn verifiers_get_verification_work_first() {
    let h = work_harness();
    for i in 0..3 {
        h.store
            .put_verify(&VerifyItem {
                engine: 1,
                sort: format!("000{i}#b-x.{i}"),
                division: Division::OneVsOne,
                seed: 42 + i,
                bots: vec![2, 1],
                rounds: 3,
                runs: vec![Run {
                    uid: "g9".into(),
                    hash: Some("a".repeat(64)),
                    record: vec![],
                    held: false,
                    colors: Vec::new(),
                }],
                uids: vec!["g9".into()],
                stage: 1,
                claimant: None,
                claim_until: None,
                kind: VerifyKind::Check,
                ttl: h.now() + VERIFY_TTL,
            })
            .unwrap();
    }
    let newbie = h.user(1, 10);
    assert!(
        work(&h, &newbie, 4)
            .battles
            .iter()
            .all(|b| b.kind == BattleKind::Normal)
    );
    let verifier = h.user(2, 60);
    let batch = work(&h, &verifier, 4);
    let kinds: Vec<BattleKind> = batch.battles.iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds,
        [
            BattleKind::Verify,
            BattleKind::Verify,
            BattleKind::Normal,
            BattleKind::Normal
        ]
    );
    assert_eq!(batch.battles[0].seed, 42);
    let vids: Vec<u32> = batch.battles[0].bots.iter().map(|b| b.vid).collect();
    assert_eq!(vids, [2, 1]);
    let item = h.store.get_verify(1, "0000#b-x.0").unwrap().unwrap();
    assert_eq!(item.claimant.as_deref(), Some("g2"));
}

#[test]
fn results_slots_and_recordings() {
    let h = work_harness();
    // A recorder (rep ≥ 20): battles with the top bot (vid 1) fill slots.
    let token = h.user(1, 30);
    let batch = work(&h, &token, 6);
    let mut post = results(&batch);
    // One invalid battle: a round missing.
    post.battles[5].rounds.as_mut().unwrap().pop();
    let response = h.post("results", &token, &post);
    assert_eq!(response.status, 200, "{:?}", response.value());
    let outcome: ResultsResponse = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(outcome.accepted, vec![0, 1, 2, 3, 4]);
    assert_eq!(outcome.rejected.len(), 1);
    assert_eq!(outcome.rejected[0].index, 5);
    assert_eq!(outcome.rejected[0].error, "invalid");
    assert!(!outcome.duplicate);
    assert_eq!(outcome.user.battles, 5);
    // The log item holds the 5 records.
    let hour = rumble_core::store::hour_key(h.now());
    let log = h.store.query_log(1, hour).unwrap();
    assert_eq!((log.len(), log[0].battles), (1, 5));

    let with_top = batch.battles[..5]
        .iter()
        .filter(|b| b.bots.iter().any(|bot| bot.vid == 1))
        .count();
    assert!(with_top > 0, "the sampler gave no battle with vid 1");
    assert!(!outcome.uploads.is_empty());
    for upload in &outcome.uploads {
        assert!(upload.url.contains("/rec/e1/1v1/t.Bot"), "{}", upload.url);
        assert_eq!(upload.headers["Cache-Control"], IMMUTABLE);
        assert_eq!(upload.headers["Content-Type"], "application/octet-stream");
        let bytes = track(&batch.batch_id, upload.index, upload.round);
        assert_eq!(upload.headers["Content-Length"], bytes.len().to_string());
        assert_eq!(
            upload.headers["x-amz-checksum-sha256"],
            rumble_lambda::s3::sha256_b64(&bytes)
        );
    }
    let presigned = h.objects.presigned.lock().unwrap().clone();
    assert!(
        presigned
            .iter()
            .all(|(bucket, key, _)| *bucket == Bucket::Jars && key.starts_with("rec/e1/1v1/"))
    );

    // A repost: the same answer, fresh URLs for the unfilled slots.
    h.advance(5_000);
    let again: ResultsResponse =
        serde_json::from_slice(&h.post("results", &token, &post).body).unwrap();
    assert!(again.duplicate);
    assert_eq!(again.accepted, outcome.accepted);
    assert_eq!(again.rejected, outcome.rejected);
    assert_eq!(again.uploads.len(), outcome.uploads.len());
    assert_eq!(h.store.query_log(1, hour).unwrap().len(), 1, "counted once");

    // recordings/done: one missing, one with the wrong bytes, the rest uploaded.
    let keys_of: Vec<String> = presigned.iter().map(|(_, key, _)| key.clone()).collect();
    let slots: Vec<String> = outcome.uploads.iter().map(|u| u.slot.clone()).collect();
    assert!(slots.len() >= 2);
    for (i, upload) in outcome.uploads.iter().enumerate().skip(1) {
        let key = &keys_of[i];
        let bytes = if i == 1 {
            b"wrong".to_vec()
        } else {
            track(&batch.batch_id, upload.index, upload.round)
        };
        h.objects.upload(Bucket::Jars, key, &bytes);
    }
    let done = h.post(
        "recordings/done",
        &token,
        &RecordingsDoneRequest {
            batch: batch.batch.clone(),
            slots: slots.clone(),
        },
    );
    assert_eq!(done.status, 200, "{:?}", done.value());
    let done = done.value();
    assert_eq!(
        done["failed"][0],
        json!({"slot": slots[0], "error": "missing"})
    );
    assert_eq!(
        done["failed"][1],
        json!({"slot": slots[1], "error": "checksum"})
    );
    assert_eq!(done["filled"].as_array().unwrap().len(), slots.len() - 2);

    // Another user can't confirm this batch's slots.
    let other = h.user(2, 30);
    let foreign = h.post(
        "recordings/done",
        &other,
        &RecordingsDoneRequest {
            batch: batch.batch.clone(),
            slots,
        },
    );
    assert_eq!(foreign.status, 403);
}

#[test]
fn late_and_foreign_results() {
    let h = work_harness();
    let token = h.user(1, 0);
    let batch = work(&h, &token, 2);
    let post = results(&batch);
    // Another user's batch.
    let other = h.user(2, 0);
    assert_eq!(h.post("results", &other, &post).status, 403);
    // Past the deadline (180 minutes).
    h.advance(181 * 60_000);
    let late = h.post("results", &token, &post);
    assert_eq!(
        (late.status, late.value()["error"].as_str()),
        (409, Some("expired"))
    );
    // A wrong engine.
    h.advance(5_000);
    let mut wrong = post.clone();
    wrong.engine_version = 7;
    assert_eq!(
        h.post("results", &token, &wrong).value()["error"],
        "engine_mismatch"
    );
}

#[test]
fn results_without_slots_for_new_users() {
    let h = work_harness();
    let token = h.user(1, 0);
    let batch = work(&h, &token, 3);
    let mut post = results(&batch);
    post.battles[2].status = BattleStatus::NotRun;
    post.battles[2].robots = None;
    post.battles[2].rounds = None;
    let outcome: ResultsResponse =
        serde_json::from_slice(&h.post("results", &token, &post).body).unwrap();
    assert_eq!(outcome.accepted, vec![0, 1, 2]);
    assert!(outcome.uploads.is_empty(), "rep 0 is below recorderMin");
    assert_eq!(outcome.user.battles, 2);
    assert_eq!(h.get("me", &token).value()["battles"], 2);
}

#[test]
fn web_sign_in() {
    let h = Harness::new(None);
    h.github.add_code("code-1", "gho_web", profile(77, "webby"));
    let login = h.send(Request::new("GET", "/api/v1/auth/login?return=%2Faccount"));
    assert_eq!(login.status, 302);
    let location = login.get("location").unwrap().to_string();
    assert!(location.starts_with("https://github.com/login/oauth/authorize?client_id=Ov23test&"));
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Frumble.example%2Fapi%2Fv1%2Fauth%2Fcallback")
    );
    assert!(location.contains("allow_signup=true"));
    let nonce = cookie_of(&login, OAUTH_COOKIE).unwrap();
    assert!(login.get("set-cookie").unwrap().contains("Max-Age=600"));
    let state = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = |cookie: &str| {
        h.send(
            Request::new(
                "GET",
                &format!(
                    "/api/v1/auth/callback?code=code-1&state={}",
                    urlencode(&state)
                ),
            )
            .header("Cookie", &format!("{OAUTH_COOKIE}={cookie}")),
        )
    };
    // The state must belong to this browser.
    assert_eq!(callback("other").status, 403);
    let done = callback(&nonce);
    assert_eq!(done.status, 302, "{:?}", done.value());
    assert_eq!(done.get("location"), Some("https://rumble.example/account"));
    let session = cookie_of(&done, SESSION_COOKIE).unwrap();
    assert!(session.starts_with("rss_"));
    assert_eq!(cookie_of(&done, OAUTH_COOKIE).as_deref(), Some(""));
    assert!(h.github.deleted().contains(&"gho_web".to_string()));
    // A forged state fails.
    let forged = h.send(
        Request::new("GET", "/api/v1/auth/callback?code=code-1&state=e30.e30")
            .header("Cookie", &format!("{OAUTH_COOKIE}={nonce}")),
    );
    assert_eq!(forged.status, 400);

    // The session works for GETs; cookie POSTs need the site's Origin (CSRF).
    let with_cookie = |method: &str, path: &str| {
        Request::new(method, &format!("/api/v1/{path}"))
            .header("Cookie", &format!("{SESSION_COOKIE}={session}"))
    };
    let me = h.send(with_cookie("GET", "me"));
    assert_eq!(me.value()["login"], "webby");
    let no_origin = h.send(with_cookie("POST", "me").json(&json!({"country": "be"})));
    assert_eq!(
        (no_origin.status, no_origin.value()["error"].as_str()),
        (403, Some("forbidden"))
    );
    let evil = h.send(
        with_cookie("POST", "me")
            .header("Origin", "https://evil.example")
            .json(&json!({"country": "be"})),
    );
    assert_eq!(evil.status, 403);
    let ok = h.send(
        with_cookie("POST", "me")
            .header("Origin", SITE)
            .json(&json!({"country": "be"})),
    );
    assert_eq!(
        (ok.status, ok.value()["country"].as_str()),
        (200, Some("be"))
    );
    // Token POSTs don't need an Origin.
    let token = h.user(5, 0);
    assert_eq!(h.post("me", &token, &json!({"country": "nl"})).status, 200);
    assert_eq!(h.post("me", &token, &json!({"country": "NLD"})).status, 400);
    // The flags that are no country: the EU and the RoboWiki's own.
    assert_eq!(h.post("me", &token, &json!({"country": "eu"})).status, 200);
    assert_eq!(h.post("me", &token, &json!({"country": "un"})).status, 200);
    assert_eq!(
        h.post("me", &token, &json!({"country": "ancap"})).status,
        200
    );
    assert_eq!(
        h.post("me", &token, &json!({"country": "ANCAP"})).status,
        400
    );

    // Logout: the session is gone, the cookie cleared.
    let out = h.send(with_cookie("POST", "auth/logout").header("Origin", SITE));
    assert_eq!(out.status, 204);
    assert_eq!(cookie_of(&out, SESSION_COOKIE).as_deref(), Some(""));
    assert_eq!(h.send(with_cookie("GET", "me")).status, 401);
}

fn urlencode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[test]
fn device_flow_token_exchange() {
    let h = Harness::new(None);
    h.github.add_token("gho_cli", profile(88, "cli"));
    h.github.add_token("gho_other_app", profile(89, "other"));
    h.github
        .state
        .lock()
        .unwrap()
        .foreign
        .push("gho_other_app".into());
    let exchange = |token: &str| {
        h.send(
            Request::new("POST", "/api/v1/auth/token")
                .json(&json!({"githubToken": token, "label": "desk"})),
        )
    };
    let response = exchange("gho_cli");
    assert_eq!(response.status, 200, "{:?}", response.value());
    let body = response.value();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("rsu_") && token.len() == 47);
    assert_eq!(body["user"]["uid"], "g88");
    assert_eq!(body["user"]["tier"], "new");
    assert!(h.github.deleted().contains(&"gho_cli".to_string()));
    assert_eq!(exchange("gho_other_app").status, 401);
    assert_eq!(exchange("gho_unknown").status, 401);
    // The upload token works.
    assert_eq!(h.get("me", &token).value()["login"], "cli");
}

#[test]
fn tokens_and_revocation() {
    let h = Harness::new(None);
    let first = h.user(1, 0);
    let second = h.user(1, 0);
    let listed = h.get("me/tokens", &first).value();
    let tokens = listed["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 2);
    let current: Vec<bool> = tokens
        .iter()
        .map(|t| t["current"].as_bool().unwrap())
        .collect();
    assert_eq!(current.iter().filter(|c| **c).count(), 1);
    let other_id = tokens
        .iter()
        .find(|t| !t["current"].as_bool().unwrap())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The second token was cached by an earlier use; revoking it acts at once here.
    assert_eq!(h.get("me", &second).status, 200);
    assert_eq!(
        h.post("me/tokens/revoke", &first, &json!({"id": other_id}))
            .status,
        200
    );
    assert_eq!(h.get("me", &second).status, 401);
    assert_eq!(
        h.post("me/tokens/revoke", &first, &json!({"id": "000000000000"}))
            .status,
        404
    );
    assert_eq!(
        h.post("me/tokens/revoke", &first, &json!({"id": "current"}))
            .status,
        200
    );
    assert_eq!(h.get("me", &first).status, 401);
}

#[test]
fn uploads_and_submissions() {
    let h = Harness::new(None);
    let token = h.user(1, 0);
    let bytes = jar("ann.Bot", Some("1.0"));
    let sha = sha_hex(&bytes);
    // Presigned PUT to the state bucket's uploads/.
    let url = h.post(
        "bots/upload-url",
        &token,
        &json!({"sha256": sha, "size": bytes.len()}),
    );
    assert_eq!(url.status, 200, "{:?}", url.value());
    assert_eq!(
        url.value()["headers"]["Content-Length"],
        bytes.len().to_string()
    );
    assert_eq!(
        url.value()["headers"]["Content-Type"],
        "application/java-archive"
    );
    let presigned = h.objects.presigned.lock().unwrap().clone();
    assert_eq!(presigned[0].0, Bucket::State);
    assert_eq!(presigned[0].1, format!("uploads/g1/{sha}.jar"));
    assert!(
        url.value()["url"]
            .as_str()
            .unwrap()
            .contains("X-Amz-Expires=900")
    );
    assert_eq!(
        h.post(
            "bots/upload-url",
            &token,
            &json!({"sha256": "xyz", "size": 5})
        )
        .status,
        400
    );
    let big = h.post(
        "bots/upload-url",
        &token,
        &json!({"sha256": sha, "size": 6 << 20}),
    );
    assert_eq!(
        (big.status, big.value()["error"].as_str()),
        (413, Some("too_large"))
    );

    // Submitting before the upload.
    let submit = json!({"source": "upload", "sha256": sha, "divisions": ["1v1"]});
    assert_eq!(h.post("bots/submit", &token, &submit).status, 400);
    h.objects
        .upload(Bucket::State, &format!("uploads/g1/{sha}.jar"), &bytes);
    let response = h.post("bots/submit", &token, &submit);
    assert_eq!(response.status, 200, "{:?}", response.value());
    let body = response.value();
    assert_eq!(body["bot"], "ann.Bot");
    assert_eq!(body["state"], "probation");
    assert_eq!(body["weightClass"], "nano");
    let published = h
        .objects
        .object(Bucket::Jars, &format!("jars/{sha}.jar"))
        .unwrap();
    assert_eq!(published.bytes, bytes);
    assert_eq!(published.cache_control.as_deref(), Some(IMMUTABLE));
    assert!(
        h.objects
            .object(Bucket::State, &format!("uploads/g1/{sha}.jar"))
            .is_none()
    );
    let mine = h.get("me/bots", &token).value();
    assert_eq!(mine["bots"][0]["id"], "ann.Bot");
    assert_eq!(mine["bots"][0]["versions"][0]["state"], "probation");

    h.advance(10_000);
    // By URL; an invalid jar; a too-large download.
    let v2 = jar("ann.Bot", Some("2.0"));
    h.fetcher
        .bodies
        .lock()
        .unwrap()
        .insert("https://x.example/b.jar".into(), v2.clone());
    h.fetcher
        .bodies
        .lock()
        .unwrap()
        .insert("https://x.example/bad.jar".into(), b"no zip".to_vec());
    h.fetcher
        .bodies
        .lock()
        .unwrap()
        .insert("https://x.example/big.jar".into(), vec![0; (5 << 20) + 1]);
    let by_url = |url: &str| {
        h.post(
            "bots/submit",
            &token,
            &json!({"source": "url", "url": url, "divisions": ["1v1"]}),
        )
    };
    let ok = by_url("https://x.example/b.jar");
    assert_eq!(
        (ok.status, ok.value()["version"].as_str()),
        (200, Some("2.0"))
    );
    let bad = by_url("https://x.example/bad.jar");
    assert_eq!(bad.status, 400);
    assert!(
        bad.value()["message"]
            .as_str()
            .unwrap()
            .starts_with("invalid (")
    );
    assert_eq!(by_url("https://x.example/big.jar").status, 413);
    assert_eq!(by_url("ftp://x.example/b.jar").status, 400);

    h.advance(10_000);
    // Someone else can't add versions; young accounts need approval.
    let other = h.user(2, 0);
    let v3 = jar("ann.Bot", Some("3.0"));
    let sha3 = sha_hex(&v3);
    h.objects
        .upload(Bucket::State, &format!("uploads/g2/{sha3}.jar"), &v3);
    let theirs = h.post(
        "bots/submit",
        &other,
        &json!({"source": "upload", "sha256": sha3, "divisions": ["1v1"]}),
    );
    assert_eq!(theirs.status, 403);
    h.store
        .patch_user(
            "g2",
            &rumble_core::store::UserPatch {
                github_created: Some(h.now() - 86_400),
                ..Default::default()
            },
        )
        .unwrap();
    let new_bot = jar("bob.Bot", Some("1"));
    let sha_new = sha_hex(&new_bot);
    h.objects.upload(
        Bucket::State,
        &format!("uploads/g2/{sha_new}.jar"),
        &new_bot,
    );
    let young = h.post(
        "bots/submit",
        &other,
        &json!({"source": "upload", "sha256": sha_new, "divisions": ["1v1"]}),
    );
    assert_eq!(young.status, 403);
    assert!(
        young.value()["message"]
            .as_str()
            .unwrap()
            .contains("approval")
    );

    h.advance(10_000);
    // Retire: owner only.
    assert_eq!(
        h.post("bots/retire", &other, &json!({"bot": "ann.Bot"}))
            .status,
        403
    );
    let retired = h.post("bots/retire", &token, &json!({"bot": "ann.Bot"}));
    assert_eq!(retired.value()["state"], "retired");
}

#[test]
fn admin_routes() {
    let h = Harness::new(None);
    let user = h.user(1, 0);
    let admin = h.user(ADMIN_ID, 0);
    for (method, path) in [
        ("GET", "admin/alerts"),
        ("GET", "admin/config"),
        ("POST", "admin/config"),
        ("POST", "admin/jars/upload-url"),
        ("POST", "admin/bots/import"),
        ("POST", "admin/bots/claim"),
        ("POST", "admin/bots/remove"),
        ("POST", "admin/users/update"),
    ] {
        let response = if method == "GET" {
            h.get(path, &user)
        } else {
            h.post(path, &user, &json!({}))
        };
        assert_eq!(response.status, 403, "{path}");
        assert_eq!(response.value()["error"], "forbidden");
    }

    // Config: read, replace (validated), visible in info at once.
    assert_eq!(h.get("admin/config", &admin).value(), json!({}));
    assert_eq!(
        h.post("admin/config", &admin, &json!({"paused": "yes"}))
            .status,
        400
    );
    let set = h.post(
        "admin/config",
        &admin,
        &json!({"paused": true, "minClient": "0.2.0"}),
    );
    assert_eq!(set.status, 200, "{:?}", set.value());
    assert_eq!(set.value()["paused"], true);
    assert_eq!(
        h.get("admin/config", &admin).value(),
        json!({"paused": true, "minClient": "0.2.0"})
    );
    assert_eq!(
        h.send(Request::new("GET", "/api/v1/info")).value()["paused"],
        true
    );

    h.advance(10_000);
    // Import: upload-url, then import from the upload (version from the list).
    let bytes = jar("jk.mega.Druss", None);
    let sha = sha_hex(&bytes);
    let url = h.post(
        "admin/jars/upload-url",
        &admin,
        &json!({"sha256": sha, "size": bytes.len()}),
    );
    assert!(url.value()["url"].is_string());
    h.objects.upload(
        Bucket::State,
        &format!("uploads/g{ADMIN_ID}/{sha}.jar"),
        &bytes,
    );
    let entry = json!({"bot": "jk.mega.Druss", "version": "3.1", "sha256": sha, "division": "1v1",
                       "author": "Julian", "sourceUrl": "http://x", "source": "list"});
    let without_flag = h.post("admin/bots/import", &admin, &entry);
    assert_eq!(
        without_flag.status, 400,
        "no robot.version without versionFromList"
    );
    let mut flagged = entry.clone();
    flagged["versionFromList"] = json!(true);
    let created = h.post("admin/bots/import", &admin, &flagged);
    assert_eq!(created.status, 200, "{:?}", created.value());
    assert_eq!(created.value(), json!({"vid": 1, "result": "created"}));
    assert!(
        h.objects
            .object(Bucket::Jars, &format!("jars/{sha}.jar"))
            .is_some()
    );
    // Now the jar exists: upload-url says so, and a rerun is `exists`.
    let exists = h.post(
        "admin/jars/upload-url",
        &admin,
        &json!({"sha256": sha, "size": bytes.len()}),
    );
    assert_eq!(exists.value(), json!({"exists": true}));
    assert_eq!(
        h.post("admin/bots/import", &admin, &flagged).value()["result"],
        "exists"
    );
    let mut melee = flagged.clone();
    melee["division"] = json!("melee");
    assert_eq!(
        h.post("admin/bots/import", &admin, &melee).value()["result"],
        "divisionAdded"
    );
    // A release date for it: `updated`.
    let mut dated = flagged.clone();
    dated["released"] = json!(1_100_000_000_000u64);
    assert_eq!(
        h.post("admin/bots/import", &admin, &dated).value()["result"],
        "updated"
    );
    // Historic versions (§16.6): the bot must exist, and its jar is not published otherwise.
    let old = jar("jk.mega.Druss", Some("2.0"));
    let old_sha = sha_hex(&old);
    h.objects.upload(
        Bucket::State,
        &format!("uploads/g{ADMIN_ID}/{old_sha}.jar"),
        &old,
    );
    let historic = json!({"bot": "jk.mega.Druss", "version": "2.0", "sha256": old_sha,
                          "division": "1v1", "historic": true, "released": 1_000_000_000_000u64});
    let mut unknown = historic.clone();
    unknown["bot"] = json!("no.Such");
    assert_eq!(h.post("admin/bots/import", &admin, &unknown).status, 404);
    assert!(
        h.objects
            .object(Bucket::Jars, &format!("jars/{old_sha}.jar"))
            .is_none()
    );
    let registered = h.post("admin/bots/import", &admin, &historic);
    assert_eq!(registered.value(), json!({"vid": 2, "result": "created"}));
    let v = h.store.get_version(2).unwrap().unwrap();
    assert_eq!(
        (v.state, v.retired, v.released),
        (
            VersionState::Retired,
            Some(1_000_000_000),
            Some(1_000_000_000_000)
        )
    );
    assert_eq!(
        h.store.get_bot("jk.mega.Druss").unwrap().unwrap().current,
        Some(1)
    );

    h.advance(10_000);
    // Claim by GitHub login (the user must have signed in) or uid.
    h.github.add_token("gho_x", profile(1, "user1"));
    let claimed = h.post(
        "admin/bots/claim",
        &admin,
        &json!({"bot": "jk.mega.Druss", "github": "user1"}),
    );
    assert_eq!(
        (claimed.status, claimed.value()["owner"].as_str()),
        (200, Some("g1"))
    );
    assert_eq!(
        h.post(
            "admin/bots/claim",
            &admin,
            &json!({"bot": "jk.mega.Druss", "github": "nobody"})
        )
        .status,
        404
    );
    assert_eq!(
        h.get("me/bots", &user).value()["bots"][0]["id"],
        "jk.mega.Druss"
    );

    // Users: make a verifier.
    let updated = h.post(
        "admin/users/update",
        &admin,
        &json!({"uid": "g1", "rep": 1000}),
    );
    assert_eq!(
        (updated.status, updated.value()["tier"].as_str()),
        (200, Some("verifier"))
    );
    assert_eq!(h.get("me", &user).value()["reputation"], 1000);
    assert_eq!(
        h.post(
            "admin/users/update",
            &admin,
            &json!({"uid": "g1", "state": "zombie"})
        )
        .status,
        400
    );

    h.advance(10_000);
    // Takedown, then the alert.
    let removed = h.post(
        "admin/bots/remove",
        &admin,
        &json!({"bot": "jk.mega.Druss", "reason": "request"}),
    );
    assert_eq!(removed.value()["state"], "removed");
    let alerts = h.get("admin/alerts", &admin).value();
    assert_eq!(alerts["alerts"][0]["kind"], "takedown");
    assert_eq!(alerts["alerts"][0]["ref"], "jk.mega.Druss");

    // A banned user's token fails with 403.
    h.post(
        "admin/users/update",
        &admin,
        &json!({"uid": "g1", "state": "ban"}),
    );
    let banned = h.get("me", &user);
    assert_eq!(banned.status, 403);
}

#[test]
fn config_reload_and_cache() {
    let h = Harness::new(Some(json!({"minClient": "0.3.0"})));
    let info = || h.send(Request::new("GET", "/api/v1/info").header("X-Rumble-Client", CLIENT));
    assert_eq!(info().status, 426);
    h.objects
        .put(
            Bucket::State,
            "state/config.json",
            br#"{"minClient": "0.2.0"}"#,
            "application/json",
            None,
        )
        .unwrap();
    // Cached for 5 minutes.
    h.advance(60_000);
    assert_eq!(info().status, 426);
    h.advance(5 * 60_000);
    assert_eq!(info().status, 200);
    // A broken file keeps the previous configuration.
    h.objects
        .put(
            Bucket::State,
            "state/config.json",
            b"{broken",
            "application/json",
            None,
        )
        .unwrap();
    h.advance(5 * 60_000);
    assert_eq!(info().status, 200);
}
