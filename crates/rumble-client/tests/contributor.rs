// SPDX-License-Identifier: EPL-2.0

//! The contributor loop (docs/rumble-backend.md §13.3) against a fake rumble server, with
//! real battles of the sample robots: a batch runs, is posted, its tracks go up with exactly
//! the headers the server gave and are confirmed; a pending batch survives a restart; a stop
//! reports what didn't run as `notRun`; an engine mismatch refuses to start. The tests with
//! battles need Java and the built robots (`./gradlew :clients:java-compat:installDist
//! :clients:java-determinism:jar :robots:samples:jar`), so they are ignored by default: run
//! them with `cargo test -p robocode-s-rumble-client --test contributor -- --ignored`
//! (`ROBOCODES_JAVA` names the JDK to use).

mod support;

use robocode_s_engine::ENGINE_VERSION;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::Environment;
use robocode_s_server::DETERMINISTIC_RULES;
use rumble_client::battle::Options;
use rumble_client::contributor::{Config, Contributor, Event, Fatal, Stop};
use rumble_client::http::sha256_hex;
use rumble_client::tokenfile::iso;
use rumble_core::api::RECORDINGS_DONE_MAX_SLOTS;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::{Req, Resp, Server};

/// Real battles one at a time: parallel JVM startups make robots late.
static SERIAL: Mutex<()> = Mutex::new(());

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn samples_jar() -> PathBuf {
    std::path::absolute(root().join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar"))
        .unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("rumble-contributor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The tree's robot host, with a data folder of its own (the cache, the pending batches).
fn environment(data: &Path, sandbox: Sandbox, find_java: bool) -> Environment {
    let root = root();
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.sandbox = sandbox;
    // Find (or download) Java in the shared data folder, once; the battles use their own.
    if find_java && let Ok(host) = env.host(true) {
        env.java = Some(host.java);
    }
    env.data = data.to_path_buf();
    env
}

fn options(data: &Path, slots: usize) -> Options {
    let sandbox = Sandbox::default();
    let mut options = Options::new(environment(data, sandbox, true));
    options.allow_unsandboxed = sandbox == Sandbox::None;
    options.slots = slots;
    options
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        for _ in chunk.len()..3 {
            out.push('=');
        }
    }
    out
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// A rumble that hands out one batch of sample battles and takes what comes back.
struct Fake {
    engine: u32,
    battles: usize,
    rounds: u32,
    jar: Vec<u8>,
    /// What `GET jars/..` answers (the jar, or something else).
    served: Vec<u8>,
    base: OnceLock<String>,
    /// Answer `POST results` with 503 (the results are still "taken").
    results_down: AtomicBool,
    batches_served: AtomicUsize,
    results: Mutex<Vec<Value>>,
    results_hits: AtomicUsize,
    seen: Mutex<HashSet<String>>,
    puts: Mutex<Vec<Req>>,
    done: Mutex<Vec<Value>>,
    /// 401 for `GET me`.
    unauthorized: AtomicBool,
}

impl Fake {
    fn new(battles: usize, rounds: u32) -> Fake {
        let jar = std::fs::read(samples_jar()).unwrap_or_default();
        Fake {
            engine: ENGINE_VERSION,
            battles,
            rounds,
            served: jar.clone(),
            jar,
            base: OnceLock::new(),
            results_down: AtomicBool::new(false),
            batches_served: AtomicUsize::new(0),
            results: Mutex::new(Vec::new()),
            results_hits: AtomicUsize::new(0),
            seen: Mutex::new(HashSet::new()),
            puts: Mutex::new(Vec::new()),
            done: Mutex::new(Vec::new()),
            unauthorized: AtomicBool::new(false),
        }
    }

    fn start(self) -> (Arc<Fake>, Server) {
        let fake = Arc::new(self);
        let handler = fake.clone();
        let server = Server::start(move |req| handler.handle(req));
        fake.base.set(server.url("")).unwrap();
        (fake, server)
    }

    fn work(&self) -> Value {
        let sha = sha256_hex(&self.jar);
        let bot = |id: &str, vid: u32| {
            json!({"id": id, "version": "", "vid": vid, "sha256": sha, "size": self.jar.len(),
                   "url": format!("/jars/{sha}.jar"), "data": null})
        };
        let battles: Vec<Value> = (0..self.battles)
            .map(|i| {
                json!({"index": i, "kind": "normal", "division": "1v1", "seed": (1000 + i).to_string(),
                       "rules": {"rounds": self.rounds, "width": 800, "height": 600,
                                 "gunCoolingRate": 0.1, "inactivityTime": 450,
                                 "sentryBorder": 100, "hideEnemyNames": false},
                       "bots": [bot("sample.Walls", 1), bot("sample.Crazy", 2)]})
            })
            .collect();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        json!({"batch": "tok.en", "batchId": "b-test1", "engineVersion": self.engine,
               "deadline": iso(now + 3 * 3600), "battles": battles})
    }

    fn handle(&self, req: &Req) -> Resp {
        let path = req.path.split('?').next().unwrap();
        match (req.method.as_str(), path) {
            ("GET", "/api/v1/info") => Resp::json(
                200,
                &json!({"api": 1, "engineVersion": self.engine, "minClient": "0.0.1",
                        "paused": false, "githubClientId": "x",
                        "deterministicRules": DETERMINISTIC_RULES,
                        "divisions": [{"id": "1v1", "rounds": 35,
                                       "battlefield": {"width": 800, "height": 600}, "robots": 2}],
                        "downloads": "https://example.invalid/download"}),
            ),
            ("GET", "/api/v1/me") if self.unauthorized.load(Ordering::SeqCst) => Resp::json(
                401,
                &json!({"error": "unauthorized", "message": "unknown token"}),
            ),
            ("GET", "/api/v1/me") => Resp::json(
                200,
                &json!({"uid": "g1", "login": "nick", "name": "Nick", "country": "be",
                        "reputation": 3, "tier": "new", "verifyRate": 0.5, "battles": 2,
                        "verifiedMatches": 0, "mismatches": 0, "admin": false,
                        "since": "2026-10-24T00:00:00Z"}),
            ),
            ("GET", "/api/v1/work") => {
                if self.batches_served.fetch_add(1, Ordering::SeqCst) == 0 {
                    Resp::json(200, &self.work())
                } else {
                    Resp::status(204).with_header("Retry-After", "1")
                }
            }
            ("GET", p) if p.starts_with("/jars/") => Resp::bytes(&self.served),
            ("POST", "/api/v1/results") => self.results(req),
            ("PUT", p) if p.starts_with("/s3/") => {
                self.puts.lock().unwrap().push(req.clone());
                Resp::status(200)
            }
            ("POST", "/api/v1/recordings/done") => {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let slots = body["slots"].clone();
                self.done.lock().unwrap().push(body);
                if slots.as_array().unwrap().len() > RECORDINGS_DONE_MAX_SLOTS {
                    return Resp::json(
                        400,
                        &json!({"error": "bad_request", "message": "too many slots"}),
                    );
                }
                Resp::json(200, &json!({"filled": slots, "failed": []}))
            }
            _ => Resp::json(404, &json!({"error": "not_found", "message": req.path})),
        }
    }

    fn results(&self, req: &Req) -> Resp {
        self.results_hits.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            req.header("x-amz-content-sha256"),
            Some(sha256_hex(&req.body).as_str())
        );
        assert_eq!(req.header("x-rumble-token"), Some("rsu_test"));
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let first = self.seen.lock().unwrap().insert(body["batch"].to_string());
        if first {
            self.results.lock().unwrap().push(body.clone());
        }
        if self.results_down.load(Ordering::SeqCst) {
            return Resp::status(503).with_header("Retry-After", "1");
        }
        let base = self.base.get().unwrap();
        let mut uploads = Vec::new();
        let mut accepted = Vec::new();
        for battle in body["battles"].as_array().unwrap() {
            accepted.push(battle["index"].clone());
            for (round, r) in battle["rounds"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let claim = &r["track"];
                let slot = format!("1v1.{}.{round}", battle["index"]);
                uploads.push(json!({
                    "slot": slot, "index": battle["index"], "round": round,
                    "url": format!("{base}/s3/{slot}"),
                    "headers": {"Content-Length": claim["size"].to_string(),
                                "x-amz-checksum-sha256":
                                    base64(&hex_to_bytes(claim["sha256"].as_str().unwrap())),
                                "Content-Type": "application/octet-stream",
                                "x-custom-signed": "yes"},
                    "expires": "2030-01-01T00:00:00Z"}));
            }
        }
        Resp::json(
            200,
            &json!({"accepted": accepted, "rejected": [], "duplicate": !first, "uploads": uploads,
                    "user": {"battles": 5, "reputation": 4, "tier": "new"}}),
        )
    }
}

/// A configuration for the tests that never start a robot: no Java is looked for.
fn light(server: &Server, data: &Path, slots: usize) -> Config {
    let mut options = Options::new(environment(data, Sandbox::None, false));
    options.allow_unsandboxed = true;
    options.slots = slots;
    let mut config = Config::new(&server.url(""), "rsu_test", options);
    config.max_batches = Some(1);
    config
}

fn config(server: &Server, data: &Path, slots: usize) -> Config {
    let mut config = Config::new(&server.url(""), "rsu_test", options(data, slots));
    config.max_batches = Some(1);
    config
}

fn pending_files(data: &Path) -> Vec<String> {
    let dir = data.join("rumble").join("pending");
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_engine_mismatch_refuses_to_start() {
    let mut fake = Fake::new(1, 1);
    fake.engine = ENGINE_VERSION + 1;
    let (_fake, server) = fake.start();
    let data = temp("mismatch");
    let config = light(&server, &data, 1);
    let Err(Fatal::Engine(why)) = Contributor::new(config) else {
        panic!("started");
    };
    assert!(why.contains("engine version"), "{why}");
    // Nothing but `info` was asked.
    assert!(server.requests().iter().all(|r| r.path == "/api/v1/info"));
}

#[test]
fn a_java_other_than_the_pinned_one_refuses_to_start() {
    let (_fake, server) = Fake::new(1, 1).start();
    let data = temp("java");
    // A Java runtime of another build of the same version (a distribution's OpenJDK).
    let home = data.join("other-jdk");
    std::fs::create_dir_all(home.join("bin")).unwrap();
    std::fs::write(home.join("bin").join("java"), "").unwrap();
    std::fs::write(home.join("release"), "IMPLEMENTOR=\"Ubuntu\"\n").unwrap();
    let mut config = light(&server, &data, 1);
    config.options.env.java = Some(home.join("bin").join("java"));
    config.options.env.sandbox = Sandbox::None;
    let Err(Fatal::Host(why)) = Contributor::new(config) else {
        panic!("started");
    };
    assert!(why.contains("Temurin-"), "{why}");
}

#[test]
fn an_unknown_token_asks_for_a_login() {
    let (fake, server) = Fake::new(1, 1).start();
    fake.unauthorized.store(true, Ordering::SeqCst);
    let data = temp("login");
    let config = light(&server, &data, 1);
    assert_eq!(Contributor::new(config).unwrap_err(), Fatal::LoginNeeded);
}

#[test]
fn melee_alone_needs_five_slots() {
    let (_fake, server) = Fake::new(1, 1).start();
    let data = temp("slots");
    let mut config = light(&server, &data, 2);
    config.divisions = vec![rumble_core::ids::Division::Melee];
    let Err(Fatal::NoDivisions(why)) = Contributor::new(config) else {
        panic!("started");
    };
    assert!(why.contains("5 parallel slots"), "{why}");
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_batch_runs_is_posted_uploaded_and_confirmed() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (fake, server) = Fake::new(3, 2).start();
    let data = temp("batch");
    let contributor = Contributor::new(config(&server, &data, 2)).unwrap();
    let mut events = Vec::new();
    let summary = contributor.run(&Stop::new(), |e| events.push(e));
    assert_eq!(summary.fatal, None);
    assert_eq!(summary.battles_ok, 3, "{events:#?}");
    assert_eq!(summary.batches_posted, 1);
    assert_eq!(summary.accepted, 3);
    assert_eq!(summary.uploaded, 6);

    // One post, three ok battles of two rounds, each with the track's claim.
    let results = fake.results.lock().unwrap().clone();
    assert_eq!(results.len(), 1);
    let post = &results[0];
    assert_eq!(post["batch"], "tok.en");
    assert_eq!(post["engineVersion"], ENGINE_VERSION);
    let battles = post["battles"].as_array().unwrap();
    assert_eq!(battles.len(), 3);
    for (i, b) in battles.iter().enumerate() {
        assert_eq!(b["index"], i);
        assert_eq!(b["status"], "ok", "{b}");
        assert_eq!(b["rounds"].as_array().unwrap().len(), 2);
        assert_eq!(b["robots"].as_array().unwrap().len(), 2);
    }

    // Every track went up with exactly the given headers, and its bytes are the claim.
    let puts = fake.puts.lock().unwrap().clone();
    assert_eq!(puts.len(), 6);
    for put in &puts {
        let slot = put.path.trim_start_matches("/s3/");
        let (index, round) = slot.trim_start_matches("1v1.").split_once('.').unwrap();
        let claim = &battles[index.parse::<usize>().unwrap()]["rounds"]
            [round.parse::<usize>().unwrap()]["track"];
        assert_eq!(put.body.len() as u64, claim["size"].as_u64().unwrap());
        assert_eq!(sha256_hex(&put.body), claim["sha256"].as_str().unwrap());
        assert_eq!(
            put.header("content-length"),
            Some(put.body.len().to_string().as_str())
        );
        assert_eq!(
            put.header("x-amz-checksum-sha256"),
            Some(base64(&hex_to_bytes(claim["sha256"].as_str().unwrap())).as_str())
        );
        assert_eq!(put.header("content-type"), Some("application/octet-stream"));
        assert_eq!(put.header("x-custom-signed"), Some("yes"));
        // Not the rumble's token: S3 gets only what was signed.
        assert_eq!(put.header("x-rumble-token"), None);
    }

    // Confirmed, then forgotten.
    let done = fake.done.lock().unwrap().clone();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["batch"], "tok.en");
    assert_eq!(done[0]["slots"].as_array().unwrap().len(), 6);
    assert_eq!(pending_files(&data), Vec::<String>::new());

    // The events a UI shows.
    assert!(matches!(
        events.first(),
        Some(Event::Batch { battles: 3, .. })
    ));
    let started = events
        .iter()
        .filter(|e| matches!(e, Event::BattleStarted { .. }))
        .count();
    let finished = events
        .iter()
        .filter(|e| matches!(e, Event::BattleDone { .. }))
        .count();
    assert_eq!((started, finished), (3, 3));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Round { n: 2, of: 2, .. }))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Posted {
            accepted: 3,
            rejected: 0,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Uploaded { n: 6, .. }))
    );
    let log = std::fs::read_to_string(data.join("rumble/log.txt")).unwrap();
    assert!(log.contains("posted b-test1"), "{log}");
    // The jars are cached under their hash.
    let sha = sha256_hex(&fake.jar);
    assert!(data.join("rumble/jars").join(sha).is_dir());
}

#[test]
#[ignore = "needs Java and the built robots"]
fn more_recordings_than_one_confirmation_takes_are_confirmed_in_chunks() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let rounds = RECORDINGS_DONE_MAX_SLOTS as u32 + 1;
    let (fake, server) = Fake::new(1, rounds).start();
    let data = temp("chunks");
    let contributor = Contributor::new(config(&server, &data, 1)).unwrap();
    let mut events = Vec::new();
    let summary = contributor.run(&Stop::new(), |e| events.push(e));
    assert_eq!(summary.fatal, None);
    assert_eq!(summary.uploaded, rounds, "{events:#?}");
    let done = fake.done.lock().unwrap().clone();
    let sizes: Vec<usize> = done
        .iter()
        .map(|d| d["slots"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, vec![RECORDINGS_DONE_MAX_SLOTS, 1]);
    assert!(
        !events.iter().any(|e| matches!(e, Event::Error { .. })),
        "{events:#?}"
    );
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_pending_batch_survives_a_restart() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let fake = Fake::new(2, 1);
    fake.results_down.store(true, Ordering::SeqCst);
    let (fake, server) = fake.start();
    let data = temp("restart");

    // The first run can't post: the results wait on disk.
    let contributor = Contributor::new(config(&server, &data, 2)).unwrap();
    let stop = Stop::new();
    let summary = std::thread::scope(|scope| {
        let run = scope.spawn(|| contributor.run(&stop, |_| {}));
        let until = std::time::Instant::now() + Duration::from_secs(120);
        while fake.results_hits.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < until, "never posted");
            std::thread::sleep(Duration::from_millis(100));
        }
        // Killed, as far as the next run can tell: a second stop is an abort.
        stop.stop();
        stop.stop();
        run.join().unwrap()
    });
    assert_eq!(summary.batches_posted, 0);
    let files = pending_files(&data);
    assert!(files.contains(&"b-test1.json".to_string()), "{files:?}");
    assert!(files.contains(&"b-test1".to_string()), "{files:?}");
    drop(contributor);

    // The next run posts it first (the server saw it before: duplicate) and uploads again.
    fake.results_down.store(false, Ordering::SeqCst);
    let mut config = config(&server, &data, 2);
    config.max_batches = Some(0);
    let contributor = Contributor::new(config).unwrap();
    let mut events = Vec::new();
    let summary = contributor.run(&Stop::new(), |e| events.push(e));
    assert_eq!(summary.fatal, None);
    assert_eq!(summary.batches_posted, 1);
    assert_eq!(summary.uploaded, 2, "{events:#?}");
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Posted {
            duplicate: true,
            ..
        }
    )));
    assert_eq!(fake.puts.lock().unwrap().len(), 2);
    assert_eq!(fake.done.lock().unwrap().len(), 1);
    assert_eq!(pending_files(&data), Vec::<String>::new());
    // It was the same results both times.
    assert_eq!(fake.results.lock().unwrap().len(), 1);
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_stop_finishes_running_battles_and_reports_the_rest_as_not_run() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (fake, server) = Fake::new(5, 3).start();
    let data = temp("stop");
    let contributor = Contributor::new(config(&server, &data, 1)).unwrap();
    let stop = Stop::new();
    let mut events = Vec::new();
    let summary = contributor.run(&stop, |e| {
        if matches!(e, Event::BattleStarted { .. }) {
            stop.stop();
        }
        events.push(e);
    });
    assert_eq!(summary.fatal, None);
    assert_eq!(summary.batches_posted, 1);
    let results = fake.results.lock().unwrap().clone();
    let battles = results[0]["battles"].as_array().unwrap();
    let statuses: Vec<&str> = battles
        .iter()
        .map(|b| b["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        ["ok", "notRun", "notRun", "notRun", "notRun"],
        "{events:#?}"
    );
    // The finished battle's tracks went up.
    assert_eq!(fake.puts.lock().unwrap().len(), 3);
    assert_eq!(pending_files(&data), Vec::<String>::new());
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_jar_that_does_not_match_its_hash_fails_the_battles_that_use_it() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut fake = Fake::new(2, 1);
    fake.served = b"not the jar".to_vec();
    let (fake, server) = fake.start();
    let data = temp("hash");
    let contributor = Contributor::new(config(&server, &data, 2)).unwrap();
    let summary = contributor.run(&Stop::new(), |_| {});
    assert_eq!(summary.robot_failed, 2);
    assert_eq!(summary.batches_posted, 1);
    let results = fake.results.lock().unwrap().clone();
    for b in results[0]["battles"].as_array().unwrap() {
        assert_eq!(b["status"], "robotFailed");
        assert_eq!(b["failure"]["reason"], "load");
        assert!(
            b["failure"]["detail"]
                .as_str()
                .unwrap()
                .contains("jar hash"),
            "{b}"
        );
    }
}
