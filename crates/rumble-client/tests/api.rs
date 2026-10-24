// SPDX-License-Identifier: EPL-2.0
//! The HTTP client against a fake server.

mod support;

use rumble_client::http::{Api, Error, Work, sha256_hex};
use rumble_core::api::{
    ErrorCode, RecordingsDoneRequest, SubmitRequest, SubmitSource, UploadUrlRequest, WorkQuery,
};
use rumble_core::ids::Division;
use serde_json::{Value, json};
use std::time::Duration;
use support::{Req, Resp, Server};

pub fn me_json() -> Value {
    json!({"uid": "g1", "login": "nick", "name": "Nick", "country": "be", "reputation": 3,
           "tier": "new", "verifyRate": 0.5, "battles": 2, "verifiedMatches": 0,
           "mismatches": 0, "admin": false, "since": "2026-10-24T00:00:00Z"})
}

fn api(server: &Server, token: Option<&str>) -> Api {
    Api::new(&server.url("/api/v1"), token.map(String::from), 7)
}

fn echo(_: &Req) -> Resp {
    Resp::json(200, &json!({}))
}

#[test]
fn every_post_carries_the_body_hash_token_and_client() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/auth/token" => Resp::json(200, &json!({"token": "rsu_x", "user": me_json()})),
        "/api/v1/recordings/done" => Resp::json(200, &json!({"filled": ["a"], "failed": []})),
        "/api/v1/bots/upload-url" => Resp::json(
            200,
            &json!({"url": "u", "headers": {"A": "b"}, "expires": "2026-10-24T00:00:00Z"}),
        ),
        "/api/v1/bots/submit" => Resp::json(
            200,
            &json!({"bot": "a.B", "version": "1", "vid": 9, "state": "probation",
                    "codesize": 100, "weightClass": "mini"}),
        ),
        "/api/v1/me" => Resp::json(200, &me_json()),
        _ => echo(req),
    });
    let api = api(&server, Some("rsu_secret"));
    api.auth_token("gho_1", "my-pc").unwrap();
    api.recordings_done(&RecordingsDoneRequest {
        batch: "tok".into(),
        slots: vec!["a".into()],
    })
    .unwrap();
    api.upload_url(&UploadUrlRequest {
        sha256: "00".repeat(32),
        size: 5,
    })
    .unwrap();
    api.submit(&SubmitRequest {
        source: SubmitSource::Url,
        sha256: None,
        url: Some("https://x/y.jar".into()),
        divisions: vec![Division::OneVsOne],
        homepage: None,
    })
    .unwrap();
    api.set_country("be").unwrap();
    api.revoke_token("current").unwrap();
    api.retire("a.B").unwrap();
    api.admin_claim("a.B", "nick").unwrap();
    api.admin_remove("a.B", "why").unwrap();
    api.admin_set_config(&json!({"x": 1})).unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 10);
    let version = env!("CARGO_PKG_VERSION");
    for req in &requests {
        assert_eq!(req.method, "POST", "{}", req.path);
        assert_eq!(
            req.header("x-amz-content-sha256"),
            Some(sha256_hex(&req.body).as_str()),
            "{}",
            req.path
        );
        assert!(!req.body.is_empty());
        assert_eq!(req.header("x-rumble-token"), Some("rsu_secret"));
        assert_eq!(
            req.header("x-rumble-client").unwrap(),
            format!(
                "robocode-s/{version} (engine 7; {}-{})",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
        assert_eq!(req.header("content-type"), Some("application/json"));
    }
    // The body is the camelCase JSON of the request.
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(first, json!({"githubToken": "gho_1", "label": "my-pc"}));
}

#[test]
fn gets_send_token_and_client_only_when_there_is_a_token() {
    let server = Server::start(|req| {
        if req.path == "/api/v1/me" {
            Resp::json(200, &me_json())
        } else {
            Resp::json(
                200,
                &json!({"api": 1, "engineVersion": 1, "minClient": "0.1.0", "paused": false,
                        "githubClientId": "cid", "deterministicRules": 1,
                        "divisions": [{"id": "1v1", "rounds": 35,
                                       "battlefield": {"width": 800, "height": 600}, "robots": 2}],
                        "downloads": "https://x/download"}),
            )
        }
    });
    let info = api(&server, None).info().unwrap();
    assert_eq!(info.github_client_id, "cid");
    assert_eq!(api(&server, Some("rsu_t")).me().unwrap().login, "nick");
    let requests = server.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].header("x-rumble-token"), None);
    assert!(requests[0].header("x-rumble-client").is_some());
    assert_eq!(requests[1].header("x-rumble-token"), Some("rsu_t"));
}

#[test]
fn work_query_and_204() {
    let server = Server::start(|req| {
        if req.path.contains("max=1&") {
            Resp::status(204).with_header("Retry-After", "600")
        } else {
            Resp::status(204)
        }
    });
    let api = api(&server, Some("t"));
    let query = |max| WorkQuery {
        engine: 1,
        max,
        divisions: vec![Division::OneVsOne, Division::Melee],
        slots: 4,
    };
    assert_eq!(
        api.work(&query(1)).unwrap(),
        Work::Wait(Some(Duration::from_secs(600)))
    );
    assert_eq!(api.work(&query(20)).unwrap(), Work::Wait(None));
    assert_eq!(
        server.requests()[0].path,
        "/api/v1/work?engine=1&max=1&divisions=1v1,melee&slots=4"
    );
}

#[test]
fn work_batch_decodes() {
    let server = Server::start(|_| {
        Resp::json(
            200,
            &json!({"batch": "tok.sig", "batchId": "b-1", "engineVersion": 1,
                    "deadline": "2026-10-24T15:00:00Z",
                    "battles": [{"index": 0, "kind": "normal", "division": "1v1",
                      "seed": "-4512334012341234",
                      "rules": {"rounds": 35, "width": 800, "height": 600, "gunCoolingRate": 0.1,
                                "inactivityTime": 450, "sentryBorder": 100,
                                "hideEnemyNames": false},
                      "bots": [{"id": "a.B", "version": "1", "vid": 1, "sha256": "00",
                                "size": 1, "url": "/jars/x.jar", "data": null}]}]}),
        )
    });
    let Work::Batch(batch) = api(&server, Some("t"))
        .work(&WorkQuery {
            engine: 1,
            max: 4,
            divisions: vec![Division::OneVsOne],
            slots: 1,
        })
        .unwrap()
    else {
        panic!("expected a batch");
    };
    assert_eq!(batch.batch_id, "b-1");
    assert_eq!(batch.battles[0].seed, -4_512_334_012_341_234);
}

fn failing(status: u16, code: &str, retry_after: Option<&str>) -> Server {
    let code = code.to_string();
    let retry_after = retry_after.map(String::from);
    Server::start(move |_| {
        let mut r = Resp::json(
            status,
            &json!({"error": code, "message": "text for humans"}),
        );
        if let Some(v) = &retry_after {
            r = r.with_header("Retry-After", v);
        }
        r
    })
}

type Check = fn(&Error) -> bool;

#[test]
fn status_table_maps_to_typed_errors() {
    let cases: [(u16, &str, Check); 11] = [
        (400, "bad_request", |e| matches!(e, Error::BadRequest(_))),
        (401, "unauthorized", |e| matches!(e, Error::Unauthorized(_))),
        (403, "forbidden", |e| matches!(e, Error::Forbidden(_))),
        (404, "not_found", |e| matches!(e, Error::NotFound(_))),
        (409, "engine_mismatch", |e| {
            matches!(e, Error::EngineMismatch(_))
        }),
        (409, "expired", |e| matches!(e, Error::Expired(_))),
        (409, "conflict", |e| matches!(e, Error::Conflict(_))),
        (413, "too_large", |e| matches!(e, Error::TooLarge(_))),
        (426, "client_outdated", |e| {
            matches!(e, Error::ClientOutdated(_))
        }),
        (429, "rate_limited", |e| {
            matches!(e, Error::RateLimited { .. })
        }),
        (503, "busy", |e| matches!(e, Error::Busy { .. })),
    ];
    for (status, code, check) in cases {
        let server = failing(status, code, None);
        let api = api(&server, Some("t"));
        for error in [api.me().unwrap_err(), api.set_country("be").unwrap_err()] {
            assert!(check(&error), "{status} {code}: {error:?}");
            assert_eq!(error.message(), "text for humans");
            assert_eq!(error.code().unwrap().as_str(), code);
            assert_eq!(error.code().unwrap().status(), status);
        }
    }
    let _ = ErrorCode::ALL;
}

#[test]
fn retry_after_is_parsed_for_429_and_503() {
    let server = failing(429, "rate_limited", Some("12"));
    let e = api(&server, Some("t")).me().unwrap_err();
    assert_eq!(e.retry_after(), Some(Duration::from_secs(12)));
    assert!(e.is_retryable());

    let server = failing(503, "busy", Some("30"));
    let e = api(&server, Some("t")).set_country("be").unwrap_err();
    assert_eq!(e.retry_after(), Some(Duration::from_secs(30)));
    assert!(matches!(e, Error::Busy { .. }));

    // Not a number: ignored, not an error.
    let server = failing(429, "rate_limited", Some("soon"));
    assert_eq!(
        api(&server, Some("t")).me().unwrap_err().retry_after(),
        None
    );

    // An unknown status stays visible.
    let server = Server::start(|_| {
        let mut r = Resp::status(502);
        r.body = b"<html>bad gateway</html>".to_vec();
        r
    });
    let e = api(&server, None).me().unwrap_err();
    assert!(matches!(e, Error::Unexpected { status: 502, .. }), "{e:?}");
    assert!(e.is_retryable());
    assert!(e.message().contains("bad gateway"));
}

#[test]
fn network_errors_are_separate() {
    // A port nobody listens on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let api = Api::new(&format!("http://127.0.0.1:{port}/api/v1"), None, 1);
    let e = api.info().unwrap_err();
    assert!(matches!(e, Error::Network(_)), "{e:?}");
    assert!(e.is_retryable());
    assert_eq!(e.code(), None);
    assert!(matches!(
        api.set_country("be").unwrap_err(),
        Error::Network(_)
    ));
}

#[test]
fn undecodable_success_is_a_decode_error() {
    let server = Server::start(|_| Resp::bytes(b"not json"));
    assert!(matches!(
        api(&server, None).info().unwrap_err(),
        Error::Decode(_)
    ));
}

#[test]
fn presigned_put_sends_exactly_the_given_headers() {
    let server = Server::start(|_| Resp::status(200));
    let api = api(&server, Some("rsu_must_not_leak"));
    let bytes = b"track bytes";
    api.put_presigned(
        &server.url("/rec/x?X-Amz-Signature=abc"),
        [
            ("Content-Length", "11"),
            ("x-amz-checksum-sha256", "Zm9v"),
            ("Content-Type", "application/octet-stream"),
        ],
        bytes,
    )
    .unwrap();
    let req = &server.requests()[0];
    assert_eq!(req.method, "PUT");
    assert_eq!(req.path, "/rec/x?X-Amz-Signature=abc");
    assert_eq!(req.body, bytes);
    assert_eq!(req.header("x-amz-checksum-sha256"), Some("Zm9v"));
    assert_eq!(req.header("content-type"), Some("application/octet-stream"));
    assert_eq!(req.header("content-length"), Some("11"));
    // Neither our token nor our client header goes to S3.
    assert_eq!(req.header("x-rumble-token"), None);
    assert_eq!(req.header("x-amz-content-sha256"), None);

    // A wrong Content-Length can't be sent.
    assert!(
        api.put_presigned(&server.url("/rec/y"), [("Content-Length", "99")], bytes)
            .is_err()
    );
    // S3 refusing is an error.
    let refusing = Server::start(|_| Resp::status(403));
    let e = api
        .put_presigned(&refusing.url("/rec/z"), [], bytes)
        .unwrap_err();
    assert!(matches!(e, Error::Unexpected { status: 403, .. }), "{e:?}");
}

#[test]
fn downloads_resolve_relative_urls_without_the_token() {
    let server = Server::start(|req| {
        if req.path == "/jars/a.jar" {
            Resp::bytes(b"JAR")
        } else {
            Resp::status(404)
        }
    });
    let api = api(&server, Some("rsu_t"));
    assert_eq!(api.download("/jars/a.jar", 100).unwrap(), b"JAR");
    assert_eq!(
        api.download(&server.url("/jars/a.jar"), 100).unwrap(),
        b"JAR"
    );
    assert!(matches!(
        api.download("/jars/none.jar", 100).unwrap_err(),
        Error::NotFound(_)
    ));
    // Over the limit.
    assert!(matches!(
        api.download("/jars/a.jar", 2).unwrap_err(),
        Error::Network(_)
    ));
    assert_eq!(server.requests()[0].header("x-rumble-token"), None);
}
