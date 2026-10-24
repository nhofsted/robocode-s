// SPDX-License-Identifier: EPL-2.0

//! Test helpers shared by the router tests and the DynamoDB Local integration test.

#![allow(dead_code)]

pub mod batch_scenario;

use rumble_core::api::{
    BattleResult, BattleStatus, ClientInfo, ResultsRequest, RobotResult, RoundResult, TrackClaim,
    WorkBattle, WorkResponse,
};
use rumble_core::auth::{self, GithubProfile};
use rumble_core::ids::{Division, WeightClass};
use rumble_core::plan::Plan;
use rumble_core::store::MemStore;
use rumble_core::store::{Reputation, Store, TokenKind, User, VersionItem, VersionState};
use rumble_core::token::Keys;
use rumble_lambda::fetch::Fetcher;
use rumble_lambda::github::FakeGithub;
use rumble_lambda::http::{Request, Response};
use rumble_lambda::routes::{App, Services, Settings};
use rumble_lambda::s3::{Bucket, MemObjects, Objects};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub const NOW_MS: u64 = 1_792_843_200_000;
pub const SITE: &str = "https://rumble.example";
pub const CLIENT: &str = "robocode-s/0.2.0 (engine 1; linux-x86_64)";
/// The default admin's GitHub id (config `admins`).
pub const ADMIN_ID: u64 = 4_036_752;

pub fn keys() -> Keys {
    Keys::parse(&format!("a:{KEY}")).unwrap()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// A class file with one method whose code has `code` bytes.
pub fn class_with_code(code: u32) -> Vec<u8> {
    let mut c = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 65];
    c.extend([0, 3]);
    c.push(1);
    c.extend([0, 4]);
    c.extend(b"Code");
    c.push(1);
    c.extend([0, 1]);
    c.extend(b"m");
    c.extend([0; 6]);
    c.extend([0, 0, 0, 0]);
    c.extend([0, 1]);
    c.extend([0, 1, 0, 2, 0, 2, 0, 1]);
    c.extend([0, 1]);
    c.extend((12 + code).to_be_bytes());
    c.extend([0, 1, 0, 1]);
    c.extend(code.to_be_bytes());
    c.extend(std::iter::repeat_n(0u8, code as usize));
    c.extend([0, 0, 0, 0]);
    c.extend([0, 0]);
    c
}

/// A valid robot jar of `class` (e.g. `my.Bot`) and `version` (none: no `robot.version`).
pub fn jar(class: &str, version: Option<&str>) -> Vec<u8> {
    let path = class.replace('.', "/");
    let mut properties = format!("robot.classname={class}\nrobot.author.name=Ann\n");
    if let Some(version) = version {
        properties.push_str(&format!("robot.version={version}\n"));
    }
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file(format!("{path}.class"), options).unwrap();
    writer.write_all(&class_with_code(100)).unwrap();
    writer
        .start_file(format!("{path}.properties"), options)
        .unwrap();
    writer.write_all(properties.as_bytes()).unwrap();
    writer.finish().unwrap().into_inner()
}

/// The bytes of a round's track (any bytes: only their hash and size matter here).
pub fn track(batch: &str, index: u32, round: u32) -> Vec<u8> {
    format!("track {batch} {index} {round}").into_bytes()
}

/// An `ok` result of a battle: robot `round % n` wins each round.
pub fn ok_result(batch: &str, battle: &WorkBattle) -> BattleResult {
    let n = battle.bots.len();
    let mut totals = vec![0.0; n];
    let mut firsts = vec![0u32; n];
    let rounds = (0..battle.rules.rounds)
        .map(|r| {
            let winner = r as usize % n;
            let scores: Vec<f64> = (0..n)
                .map(|i| if i == winner { 100.0 } else { 20.0 })
                .collect();
            for (total, s) in totals.iter_mut().zip(&scores) {
                *total += s;
            }
            firsts[winner] += 1;
            let bytes = track(batch, battle.index, r);
            RoundResult {
                turns: 500,
                winner: winner as i32,
                firsts: vec![winner as u32],
                scores,
                hash: sha_hex(format!("round {r}").as_bytes()),
                track: Some(TrackClaim {
                    sha256: sha_hex(&bytes),
                    size: bytes.len() as u64,
                }),
            }
        })
        .collect();
    BattleResult {
        index: battle.index,
        status: BattleStatus::Ok,
        duration_ms: Some(1000),
        battle_hash: Some(sha_hex(
            format!("battle {} {}", battle.seed, battle.index).as_bytes(),
        )),
        robots: Some(
            (0..n)
                .map(|i| RobotResult {
                    score: totals[i],
                    survival: 0.0,
                    last_survivor_bonus: 0.0,
                    bullet_damage: totals[i],
                    bullet_damage_bonus: 0.0,
                    ram_damage: 0.0,
                    ram_damage_bonus: 0.0,
                    firsts: firsts[i],
                    seconds: 0,
                    thirds: 0,
                    rank: i as u32 + 1,
                })
                .collect(),
        ),
        rounds: Some(rounds),
        failure: None,
    }
}

/// A results post with every battle `ok`.
pub fn results(work: &WorkResponse) -> ResultsRequest {
    ResultsRequest {
        batch: work.batch.clone(),
        engine_version: work.engine_version,
        client: ClientInfo {
            release: "0.2.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            sandbox: "os".into(),
        },
        battles: work
            .battles
            .iter()
            .map(|b| ok_result(&work.batch_id, b))
            .collect(),
    }
}

pub fn version(vid: u32) -> VersionItem {
    VersionItem {
        vid,
        bot: format!("t.Bot{vid}"),
        version: "1.0".into(),
        sha256: format!("{vid:064x}"),
        size: 1000,
        codesize: Some(600),
        class: WeightClass::Micro,
        divisions: vec![Division::OneVsOne],
        state: VersionState::Active,
        probation_battles: 0,
        probation_failures: 0,
        class_name: format!("t.Bot{vid}"),
        created: 0,
        retired: None,
        url: None,
    }
}

/// A 1v1 plan of versions 1..=3 with version 1 in the top.
pub fn plan_1v1(engine: u32) -> Plan {
    Plan {
        engine,
        division: Division::OneVsOne,
        generated_at: "2026-10-24T12:05:00Z".into(),
        hourly: 30,
        bots: (1..=3u32)
            .map(|vid| {
                let v = version(vid);
                (vid, v.bot, v.version, v.sha256, v.size)
            })
            .collect(),
        pairs: vec![(1, 2, 10), (1, 3, 10), (2, 3, 10)],
        anchors: vec![],
        partners: BTreeMap::new(),
        top: vec![1],
        slot_pairs: vec![],
        probation: vec![],
    }
}

/// Creates user `g<id>` (an old GitHub account) with reputation `rep` and an upload token.
pub fn user_with_token(store: &dyn Store, id: u64, rep: u32, random: u8, now: u64) -> String {
    let uid = format!("g{id}");
    if store.get_user(&uid).unwrap().is_none() {
        store
            .create_user(&User {
                uid: uid.clone(),
                login: format!("user{id}"),
                github_id: id,
                github_created: Some(1_300_000_000),
                created: now,
                ..User::default()
            })
            .unwrap();
        store
            .set_reputation(
                &uid,
                0,
                &Reputation {
                    rep,
                    ..Reputation::default()
                },
            )
            .unwrap();
    }
    auth::issue_token(store, &uid, TokenKind::Upload, "test", &[random; 32], now)
        .unwrap()
        .0
}

pub fn profile(id: u64, login: &str) -> GithubProfile {
    GithubProfile {
        id,
        login: login.into(),
        name: Some(format!("{login} name")),
        created_at: Some(1_300_000_000),
    }
}

/// URLs and their bodies.
#[derive(Default)]
pub struct FakeFetcher {
    pub bodies: Mutex<HashMap<String, Vec<u8>>>,
}

impl Fetcher for FakeFetcher {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        match self.bodies.lock().unwrap().get(url) {
            Some(body) if body.len() as u64 > limit => {
                Err(format!("too large: over {limit} bytes"))
            }
            Some(body) => Ok(body.clone()),
            None => Err("download failed: HTTP 404".into()),
        }
    }
}

/// The router over in-memory services, with a settable clock.
pub struct Harness {
    pub app: App,
    pub store: Arc<MemStore>,
    pub objects: Arc<MemObjects>,
    pub github: Arc<FakeGithub>,
    pub fetcher: Arc<FakeFetcher>,
    pub clock: Arc<AtomicU64>,
}

impl Harness {
    /// With `config` written to `state/config.json` first (`None`: no file).
    pub fn new(config: Option<Value>) -> Harness {
        let store = Arc::new(MemStore::new());
        let objects = Arc::new(MemObjects::new());
        if let Some(config) = config {
            objects
                .put(
                    Bucket::State,
                    "state/config.json",
                    &serde_json::to_vec(&config).unwrap(),
                    "application/json",
                    None,
                )
                .unwrap();
        }
        let github = Arc::new(FakeGithub::new());
        let fetcher = Arc::new(FakeFetcher::default());
        let clock = Arc::new(AtomicU64::new(NOW_MS));
        let counter = Arc::new(AtomicU64::new(1));
        let services = Services {
            store: Box::new(store.clone()),
            objects: Box::new(objects.clone()),
            github: Box::new(github.clone()),
            fetcher: Box::new(fetcher.clone()),
            clock: {
                let clock = clock.clone();
                Box::new(move || clock.load(Ordering::SeqCst))
            },
            random: Box::new(move || {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                Sha256::digest(n.to_le_bytes()).into()
            }),
        };
        let app = App::new(
            services,
            keys(),
            Settings {
                site_origin: SITE.into(),
                github_client_id: "Ov23test".into(),
            },
        );
        Harness {
            app,
            store,
            objects,
            github,
            fetcher,
            clock,
        }
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }

    pub fn now(&self) -> u64 {
        self.now_ms() / 1000
    }

    pub fn advance(&self, ms: u64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn put_plan(&self, plan: &Plan) {
        let key = format!(
            "state/e{}/{}/plan.json.gz",
            plan.engine,
            plan.division.as_str()
        );
        self.objects
            .put(
                Bucket::State,
                &key,
                &plan.to_bytes(),
                "application/gzip",
                None,
            )
            .unwrap();
    }

    pub fn versions(&self, vids: impl IntoIterator<Item = u32>) {
        for vid in vids {
            self.store.put_version(&version(vid)).unwrap();
        }
    }

    pub fn user(&self, id: u64, rep: u32) -> String {
        let random = (self.store.list_tokens(&format!("g{id}")).unwrap().len() as u8)
            .wrapping_mul(31)
            .wrapping_add(id as u8);
        user_with_token(self.store.as_ref(), id, rep, random, self.now())
    }

    pub fn send(&self, request: Request) -> Response {
        self.app.handle(&request)
    }

    pub fn get(&self, path: &str, token: &str) -> Response {
        self.send(
            Request::new("GET", &format!("/api/v1/{path}"))
                .header("X-Rumble-Client", CLIENT)
                .header("X-Rumble-Token", token),
        )
    }

    pub fn post(&self, path: &str, token: &str, body: &impl serde::Serialize) -> Response {
        self.send(
            Request::new("POST", &format!("/api/v1/{path}"))
                .header("X-Rumble-Client", CLIENT)
                .header("X-Rumble-Token", token)
                .json(body),
        )
    }
}
