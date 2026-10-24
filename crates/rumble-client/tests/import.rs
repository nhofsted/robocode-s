// SPDX-License-Identifier: EPL-2.0
//! The importer's flow against a fake API.

mod support;

use rumble_client::http::Api;
use rumble_client::import::{self, Fetcher, Options, parse_list};
use rumble_core::ids::Division;
use serde_json::{Value, json};
use std::sync::Mutex;
use support::{Resp, Server};

const SAMPLE_JAR: &[u8] =
    include_bytes!("../../rumble-core/tests/fixtures/samples/sample.MyFirstRobot.jar");
const OTHER_JAR: &[u8] =
    include_bytes!("../../rumble-core/tests/fixtures/samples/sample.Crazy.jar");

struct Files;

impl Fetcher for Files {
    fn fetch(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
        match url {
            "http://list/first.jar" => Ok(SAMPLE_JAR.to_vec()),
            "http://list/second.jar" => Ok(OTHER_JAR.to_vec()),
            _ => Err("HTTP 404".into()),
        }
    }
}

fn sha(bytes: &[u8]) -> String {
    rumble_core::jar::sha256_hex(bytes)
}

#[test]
fn upload_unless_exists_then_import() {
    let known = sha(OTHER_JAR);
    let puts = std::sync::Arc::new(Mutex::new(0));
    let server = {
        let puts = puts.clone();
        let base = Mutex::new(String::new());
        Server::start(move |req| {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            match (req.method.as_str(), req.path.as_str()) {
                ("POST", "/api/v1/admin/jars/upload-url") => {
                    if body["sha256"] == known.as_str() {
                        Resp::json(200, &json!({"exists": true}))
                    } else {
                        let host = req.header("host").unwrap().to_string();
                        *base.lock().unwrap() = host.clone();
                        Resp::json(
                            200,
                            &json!({"url": format!("http://{host}/s3/jar"),
                                    "headers": {"Content-Type": "application/java-archive"},
                                    "expires": "2026-10-24T00:00:00Z"}),
                        )
                    }
                }
                ("PUT", "/s3/jar") => {
                    *puts.lock().unwrap() += 1;
                    Resp::status(200)
                }
                ("POST", "/api/v1/admin/bots/import") => {
                    let result = if body["bot"] == "sample.Crazy" {
                        "exists"
                    } else {
                        "created"
                    };
                    Resp::json(200, &json!({"vid": 1, "result": result}))
                }
                _ => Resp::status(404),
            }
        })
    };
    let api = Api::new(&server.url("/api/v1"), Some("rsu_admin".into()), 1);
    let list = parse_list(
        "<pre>\nsample.MyFirstRobot 1.0,http://list/first.jar\nsample.Crazy 1.0,http://list/second.jar\nsample.Gone 1.0,http://list/gone.jar\n</pre>",
    );
    let mut options = Options::new(Division::Melee);
    options.mirror = None;
    let report = import::run(&list, &options, &Files, Some(&api), &|_, _, _| {});

    assert_eq!(*puts.lock().unwrap(), 1, "the known jar is not uploaded");
    assert_eq!(report.counts["created"], 1);
    assert_eq!(report.counts["exists"], 1);
    assert_eq!(report.counts["failed"], 1);
    assert_eq!(report.entries[0].id, "sample.MyFirstRobot");
    assert_eq!(
        report.entries[2].error.as_deref(),
        Some("download (HTTP 404)")
    );

    let imports: Vec<Value> = server
        .requests()
        .iter()
        .filter(|r| r.path == "/api/v1/admin/bots/import")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(imports.len(), 2);
    let first = imports
        .iter()
        .find(|b| b["bot"] == "sample.MyFirstRobot")
        .unwrap();
    assert_eq!(first["version"], "1.0");
    assert_eq!(first["sha256"], sha(SAMPLE_JAR).as_str());
    assert_eq!(first["division"], "melee");
    assert_eq!(first["author"], "Mathew Nelson");
    assert_eq!(first["sourceUrl"], "http://list/first.jar");
    assert_eq!(first["source"], "list");
    assert_eq!(first["versionFromList"], true);
    // Every POST carries the admin token and the body hash.
    for r in server.requests().iter().filter(|r| r.method == "POST") {
        assert_eq!(r.header("x-rumble-token"), Some("rsu_admin"));
        assert_eq!(
            r.header("x-amz-content-sha256"),
            Some(sha(&r.body).as_str())
        );
    }
}

#[test]
fn conflicts_and_dry_runs() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/admin/jars/upload-url" => Resp::json(200, &json!({"exists": true})),
        "/api/v1/admin/bots/import" => Resp::json(
            409,
            &json!({"error": "conflict", "message": "version registered with another jar"}),
        ),
        _ => Resp::status(404),
    });
    let api = Api::new(&server.url("/api/v1"), None, 1);
    let list = parse_list("sample.MyFirstRobot 1.0,http://list/first.jar\n");
    let mut options = Options::new(Division::OneVsOne);
    options.mirror = None;
    let report = import::run(&list, &options, &Files, Some(&api), &|_, _, _| {});
    assert_eq!(report.entries[0].result, "failed");
    assert_eq!(report.entries[0].error.as_deref(), Some("conflict"));

    // A dry run makes no API calls.
    let before = server.requests().len();
    options.dry_run = true;
    let report = import::run(&list, &options, &Files, None, &|_, _, _| {});
    assert_eq!(report.entries[0].result, "valid");
    assert_eq!(server.requests().len(), before);
}

#[test]
fn rate_limits_are_retried() {
    let limited = std::sync::Arc::new(Mutex::new(0));
    let server = {
        let limited = limited.clone();
        Server::start(move |req| match req.path.as_str() {
            "/api/v1/admin/jars/upload-url" => Resp::json(200, &json!({"exists": true})),
            "/api/v1/admin/bots/import" => {
                let mut n = limited.lock().unwrap();
                *n += 1;
                if *n <= 2 {
                    Resp::json(
                        429,
                        &json!({"error": {"code": "rate_limited", "message": "slow down"}}),
                    )
                    .with_header("Retry-After", "1")
                } else {
                    Resp::json(200, &json!({"vid": 1, "result": "created"}))
                }
            }
            _ => Resp::status(404),
        })
    };
    let api = Api::new(&server.url("/api/v1"), Some("rsu_admin".into()), 1);
    let list = parse_list("<pre>\nsample.MyFirstRobot 1.0,http://list/first.jar\n</pre>");
    let mut options = Options::new(Division::Melee);
    options.mirror = None;
    let report = import::run(&list, &options, &Files, Some(&api), &|_, _, _| {});
    assert_eq!(report.counts["created"], 1, "{:?}", report.entries);
    assert_eq!(*limited.lock().unwrap(), 3);
}
