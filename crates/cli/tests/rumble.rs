// SPDX-License-Identifier: EPL-2.0

//! `robocode-s rumble` against a fake rumble server (docs/rumble-backend.md §13.6): status,
//! login, logout and the refusals need nothing but the binary; `run --batches 1` runs real
//! battles of the sample robots, so it needs Java and the built robot host and sample robots
//! (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`) and is ignored by default: `cargo test -p robocode-s-cli --test
//! rumble -- --ignored` (`ROBOCODES_JAVA` names the JDK to use).

mod support;

use robocode_s_engine::ENGINE_VERSION;
use robocode_s_server::DETERMINISTIC_RULES;
use rumble_client::http::sha256_hex;
use rumble_client::tokenfile::iso;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use support::{Req, Resp, Server};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "robocode-s-cli-rumble-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn me() -> Value {
    json!({"uid": "g1", "login": "nick", "name": "Nick", "country": "be", "reputation": 3,
           "tier": "new", "verifyRate": 0.5, "battles": 2, "verifiedMatches": 0,
           "mismatches": 0, "admin": false, "since": "2026-10-24T00:00:00Z"})
}

fn info(engine: u32) -> Value {
    json!({"api": 1, "engineVersion": engine, "minClient": "0.0.1", "paused": false,
           "githubClientId": "cid", "deterministicRules": DETERMINISTIC_RULES,
           "divisions": [], "downloads": "https://example.invalid/download"})
}

/// The binary, with its settings in `dir`.
fn rumble(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_robocode-s"))
        .current_dir(root())
        .env("ROBOCODES_RUMBLE_CONFIG", dir.join("rumble.json"))
        .env_remove("ROBOCODES_RUMBLE_TOKEN")
        .env_remove("ROBOCODES_RUMBLE_SERVER")
        .env_remove("ROBOCODES_RUMBLE_GITHUB")
        .arg("rumble")
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn store_token(dir: &Path, server: &Server) {
    std::fs::write(
        dir.join("rumble.json"),
        json!({"server": server.url(""), "token": "rsu_test", "login": "nick",
               "savedAt": "2026-10-24T00:00:00Z"})
        .to_string(),
    )
    .unwrap();
}

#[test]
fn help_lists_the_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_robocode-s"))
        .args(["rumble", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = text(&out.stdout);
    for word in [
        "login",
        "logout",
        "status",
        "run",
        "admin import",
        "--batches",
    ] {
        assert!(help.contains(word), "{word}");
    }
    let top = Command::new(env!("CARGO_BIN_EXE_robocode-s"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(text(&top.stdout).contains("rumble"));
}

#[test]
fn a_bad_command_line_prints_the_usage() {
    let dir = temp("bad");
    let out = rumble(&dir, &["run", "--parallel", "0"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains("--parallel must be at least 1"), "{err}");
    assert!(err.contains("usage: robocode-s rumble"), "{err}");
}

#[test]
fn run_without_a_login_says_so() {
    let dir = temp("nologin");
    let out = rumble(&dir, &["run", "--batches", "1"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("robocode-s rumble login"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn status_reports_the_account_and_the_engine() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/info" => Resp::json(200, &info(ENGINE_VERSION)),
        "/api/v1/me" => {
            assert_eq!(req.header("x-rumble-token"), Some("rsu_test"));
            Resp::json(200, &me())
        }
        _ => Resp::status(404),
    });
    let dir = temp("status");
    store_token(&dir, &server);
    let out = rumble(&dir, &["status"]);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", text(&out.stderr));
    assert!(stdout.contains("compatible"), "{stdout}");
    assert!(!stdout.contains("NOT compatible"), "{stdout}");
    assert!(stdout.contains("nick (g1)"), "{stdout}");
    assert!(stdout.contains("2 battles"), "{stdout}");
}

#[test]
fn status_flags_another_engine() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/info" => Resp::json(200, &info(ENGINE_VERSION + 1)),
        "/api/v1/me" => Resp::json(200, &me()),
        _ => Resp::status(404),
    });
    let dir = temp("status-engine");
    store_token(&dir, &server);
    let out = rumble(&dir, &["status"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stdout).contains("NOT compatible"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn logout_revokes_the_token_and_removes_the_file() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/me/tokens/revoke" => Resp::json(200, &json!({"ok": true})),
        _ => Resp::status(404),
    });
    let dir = temp("logout");
    store_token(&dir, &server);
    let out = rumble(&dir, &["logout"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("x-rumble-token"), Some("rsu_test"));
    assert_eq!(requests[0].body_text(), r#"{"id":"current"}"#);
    assert!(!dir.join("rumble.json").exists());
    // Again: nothing to do.
    let out = rumble(&dir, &["logout"]);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("Not signed in"));
}

#[test]
fn login_goes_through_the_device_flow_and_keeps_the_token() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/info" => Resp::json(200, &info(ENGINE_VERSION)),
        "/login/device/code" => Resp::json(
            200,
            &json!({"device_code": "dc", "user_code": "ABCD-1234",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 60, "interval": 1}),
        ),
        "/login/oauth/access_token" => Resp::json(200, &json!({"access_token": "gho_x"})),
        "/api/v1/auth/token" => {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["githubToken"], "gho_x");
            Resp::json(200, &json!({"token": "rsu_new", "user": me()}))
        }
        _ => Resp::status(404),
    });
    let dir = temp("login");
    let out = Command::new(env!("CARGO_BIN_EXE_robocode-s"))
        .current_dir(root())
        .env("ROBOCODES_RUMBLE_CONFIG", dir.join("rumble.json"))
        .env("ROBOCODES_RUMBLE_GITHUB", server.url(""))
        .env_remove("ROBOCODES_RUMBLE_SERVER")
        .args(["rumble", "login", "--server", &server.url("")])
        .output()
        .unwrap();
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", text(&out.stderr));
    assert!(stdout.contains("ABCD-1234"), "{stdout}");
    assert!(stdout.contains("Signed in"), "{stdout}");
    let file: Value =
        serde_json::from_slice(&std::fs::read(dir.join("rumble.json")).unwrap()).unwrap();
    assert_eq!(file["token"], "rsu_new");
    assert_eq!(file["login"], "nick");
    assert_eq!(file["server"], server.url(""));
}

#[test]
fn run_refuses_another_engine() {
    let server = Server::start(|req| match req.path.as_str() {
        "/api/v1/info" => Resp::json(200, &info(ENGINE_VERSION + 1)),
        _ => Resp::status(404),
    });
    let dir = temp("run-engine");
    store_token(&dir, &server);
    let out = rumble(&dir, &["run", "--batches", "1", "--unsandboxed"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains("engine version"), "{err}");
    assert!(server.requests().iter().all(|r| r.path == "/api/v1/info"));
}

/// A rumble with one batch of sample battles.
fn battles_server(jar: Vec<u8>, battles: usize) -> (Server, std::sync::Arc<Mutex<Vec<Req>>>) {
    let log = std::sync::Arc::new(Mutex::new(Vec::new()));
    let served = std::sync::Arc::new(Mutex::new(0usize));
    let base = std::sync::Arc::new(std::sync::OnceLock::<String>::new());
    let (log2, base2) = (log.clone(), base.clone());
    let server = Server::start(move |req| {
        let path = req.path.split('?').next().unwrap();
        log2.lock().unwrap().push(req.clone());
        match (req.method.as_str(), path) {
            ("GET", "/api/v1/info") => Resp::json(200, &info(ENGINE_VERSION)),
            ("GET", "/api/v1/me") => Resp::json(200, &me()),
            ("GET", "/api/v1/work") => {
                let mut served = served.lock().unwrap();
                *served += 1;
                if *served > 1 {
                    return Resp::status(204).with_header("Retry-After", "1");
                }
                let sha = sha256_hex(&jar);
                let bot = |id: &str, vid: u32| {
                    json!({"id": id, "version": "", "vid": vid, "sha256": sha, "size": jar.len(),
                           "url": format!("/jars/{sha}.jar"), "data": null})
                };
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let battles: Vec<Value> = (0..battles)
                    .map(|i| {
                        json!({"index": i, "kind": "normal", "division": "1v1",
                               "seed": (7 + i).to_string(),
                               "rules": {"rounds": 2, "width": 800, "height": 600,
                                         "gunCoolingRate": 0.1, "inactivityTime": 450,
                                         "sentryBorder": 100, "hideEnemyNames": false},
                               "bots": [bot("sample.Walls", 1), bot("sample.Crazy", 2)]})
                    })
                    .collect();
                Resp::json(
                    200,
                    &json!({"batch": "tok.en", "batchId": "b-cli1", "engineVersion": ENGINE_VERSION,
                            "deadline": iso(now + 3 * 3600), "battles": battles}),
                )
            }
            ("GET", p) if p.starts_with("/jars/") => Resp::bytes(&jar),
            ("POST", "/api/v1/results") => {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let base = base2.get().unwrap();
                let mut uploads = Vec::new();
                let mut accepted = Vec::new();
                for b in body["battles"].as_array().unwrap() {
                    accepted.push(b["index"].clone());
                    for (round, r) in b["rounds"].as_array().into_iter().flatten().enumerate() {
                        let slot = format!("1v1.{}.{round}", b["index"]);
                        uploads.push(json!({
                            "slot": slot, "index": b["index"], "round": round,
                            "url": format!("{base}/s3/{slot}"),
                            "headers": {"Content-Length": r["track"]["size"].to_string()},
                            "expires": "2030-01-01T00:00:00Z"}));
                    }
                }
                Resp::json(
                    200,
                    &json!({"accepted": accepted, "rejected": [], "duplicate": false,
                            "uploads": uploads,
                            "user": {"battles": 5, "reputation": 4, "tier": "new"}}),
                )
            }
            ("PUT", p) if p.starts_with("/s3/") => Resp::status(200),
            ("POST", "/api/v1/recordings/done") => {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                Resp::json(200, &json!({"filled": body["slots"], "failed": []}))
            }
            _ => Resp::status(404),
        }
    });
    base.set(server.url("")).unwrap();
    (server, log)
}

#[test]
#[ignore = "needs Java and the built robots"]
fn run_one_batch_prints_a_line_per_battle_and_per_post() {
    let jar =
        std::fs::read(root().join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar")).unwrap();
    let (server, log) = battles_server(jar, 3);
    let dir = temp("run");
    store_token(&dir, &server);
    let java = std::env::var("ROBOCODES_JAVA").unwrap_or_else(|_| "java".into());
    let data = dir.join("data");
    let out = rumble(
        &dir,
        &[
            "run",
            "--batches",
            "1",
            "--parallel",
            "2",
            "--divisions",
            "1v1",
            "--data",
            data.to_str().unwrap(),
            "--java",
            &java,
        ],
    );
    let stdout = text(&out.stdout);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let battle_lines = stdout
        .lines()
        .filter(|l| l.starts_with("1v1 sample.Walls vs sample.Crazy: ") && l.contains('%'))
        .count();
    assert_eq!(battle_lines, 3, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("posted b-cli1: 3 accepted, 0 rejected"),
        "{stdout}"
    );
    assert!(stdout.contains("uploaded 6 recordings"), "{stdout}");
    let paths: Vec<String> = log.lock().unwrap().iter().map(|r| r.path.clone()).collect();
    assert!(paths.iter().any(|p| p == "/api/v1/results"));
    assert_eq!(paths.iter().filter(|p| p.starts_with("/s3/")).count(), 6);
    assert!(paths.iter().any(|p| p == "/api/v1/recordings/done"));
    assert!(data.join("rumble/log.txt").is_file());
    assert!(
        std::fs::read_dir(data.join("rumble/pending"))
            .map(|d| d.count() == 0)
            .unwrap_or(true)
    );

    // The same with --json: events as JSON lines.
    let (server, _) = battles_server(
        std::fs::read(root().join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar")).unwrap(),
        1,
    );
    store_token(&dir, &server);
    let out = rumble(
        &dir,
        &[
            "run",
            "--batches",
            "1",
            "--parallel",
            "1",
            "--divisions",
            "1v1",
            "--json",
            "--data",
            data.to_str().unwrap(),
            "--java",
            &java,
        ],
    );
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    let events: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
        .collect();
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"batch"));
    assert!(kinds.contains(&"battleStarted") && kinds.contains(&"round"));
    assert!(kinds.contains(&"battleDone") && kinds.contains(&"posted"));
    assert!(kinds.contains(&"uploaded"));
}
