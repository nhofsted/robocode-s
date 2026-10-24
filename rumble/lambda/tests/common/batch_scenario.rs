// SPDX-License-Identifier: EPL-2.0

//! The `batch` Lambda's scenario, run on the in-memory services (`tests/batch.rs`) and on
//! DynamoDB Local and the S3 mock (`tests/aws_local.rs`): a synthetic 50-bot 1v1 rumble
//! with three hours of result log; plan (fold, plan file, state, probation pass and
//! probation work); publish (data files checked against the core's types and, with Node,
//! against `types.ts`; unchanged bot files not rewritten); a crash between the plan file
//! and the state, rerun, folds nothing twice; a ban writes corrections (resumably, in
//! several runs) that cancel the user's battles; a takedown removes the jar, the
//! recordings and the slot item and invalidates them.

use rumble_core::api::UserUpdateRequest;
use rumble_core::bots;
use rumble_core::data::{BotDetails, Contributors, Rankings, RumbleIndex};
use rumble_core::ids::{Division, WeightClass};
use rumble_core::plan::Plan;
use rumble_core::record::{Entry, Record};
use rumble_core::state::State;
use rumble_core::store::{
    BotItem, BotSource, BotState, LOG_TTL, LogItem, Reputation, SYSTEM_UID, SlotEntry, SlotStatus,
    Store, User, UserState, VerifyKind, VersionItem, VersionState, hour_key,
};
use rumble_lambda::batch::cleanup::CleanupReport;
use rumble_lambda::batch::plan::{PlanReport, state_key};
use rumble_lambda::batch::publish::PublishReport;
use rumble_lambda::batch::{Batch, Mode, Services};
use rumble_lambda::cloudfront::MemCdn;
use rumble_lambda::s3::{Bucket, Fetched, IMMUTABLE, JAR_TYPE, Objects, TRACK_TYPE};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 2026-10-24T12:05:00Z, when the hourly plan runs.
pub const T0: u64 = 1_792_843_200 + 300;
const HOURS: [u32; 3] = [2026102409, 2026102410, 2026102411];
const BOTS: u32 = 50;

pub struct World {
    pub store: Arc<dyn Store + Send + Sync>,
    pub objects: Arc<dyn Objects>,
    pub cdn: Arc<MemCdn>,
    pub clock: Arc<AtomicU64>,
    /// Milliseconds the clock moves on every reading (for the time budget).
    pub tick: Arc<AtomicU64>,
}

impl World {
    pub fn new(store: Arc<dyn Store + Send + Sync>, objects: Arc<dyn Objects>) -> World {
        World {
            store,
            objects,
            cdn: Arc::new(MemCdn::new()),
            clock: Arc::new(AtomicU64::new(T0 * 1000)),
            tick: Arc::new(AtomicU64::new(0)),
        }
    }

    fn set(&self, unix: u64) {
        self.clock.store(unix * 1000, Ordering::SeqCst);
    }

    fn now_ms(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }

    fn batch(&self, budget_ms: u64) -> Batch {
        let clock = self.clock.clone();
        let tick = self.tick.clone();
        let counter = AtomicU64::new(1);
        let mut batch = Batch::new(Services {
            store: Box::new(self.store.clone()),
            objects: Box::new(self.objects.clone()),
            cdn: Box::new(self.cdn.clone()),
            clock: Box::new(move || clock.fetch_add(tick.load(Ordering::SeqCst), Ordering::SeqCst)),
            sleep: Box::new(|_| {}),
            random: Box::new(move || {
                Sha256::digest(counter.fetch_add(1, Ordering::SeqCst).to_le_bytes()).into()
            }),
        });
        batch.budget_ms = budget_ms;
        batch
    }

    fn plan(&self) -> PlanReport {
        let value = self.batch(u64::MAX).run(Mode::Plan).expect("plan");
        serde_json::from_value(value).expect("plan report")
    }

    fn publish(&self) -> PublishReport {
        serde_json::from_value(self.batch(u64::MAX).run(Mode::Publish).expect("publish"))
            .expect("publish report")
    }

    fn cleanup(&self, budget_ms: u64) -> CleanupReport {
        serde_json::from_value(self.batch(budget_ms).run(Mode::Cleanup).expect("cleanup"))
            .expect("cleanup report")
    }

    fn get(&self, bucket: Bucket, key: &str) -> Option<Vec<u8>> {
        match self.objects.get(bucket, key, None).unwrap() {
            Fetched::Found { bytes, .. } => Some(bytes),
            _ => None,
        }
    }

    fn json<T: for<'de> serde::Deserialize<'de>>(&self, key: &str) -> T {
        let bytes = self
            .get(Bucket::Site, key)
            .unwrap_or_else(|| panic!("{key} missing"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{key}: {e}"))
    }

    fn state(&self) -> State {
        State::from_bytes(
            &self
                .get(Bucket::State, &state_key(1, Division::OneVsOne))
                .expect("pairings.bin"),
        )
        .unwrap()
    }
}

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn class(vid: u32) -> WeightClass {
    [
        WeightClass::Nano,
        WeightClass::Micro,
        WeightClass::Mini,
        WeightClass::Mega,
    ][vid as usize % 4]
}

fn version(vid: u32, bot: u32, version: &str, state: VersionState) -> VersionItem {
    VersionItem {
        vid,
        bot: format!("syn.Bot{bot}"),
        version: version.to_string(),
        sha256: sha(&format!("jar {vid}")),
        size: 1000 + u64::from(vid),
        codesize: Some([200, 700, 1400, 3000][vid as usize % 4]),
        class: class(vid),
        divisions: vec![Division::OneVsOne],
        state,
        probation_battles: 0,
        probation_failures: 0,
        class_name: format!("syn.Bot{bot}"),
        created: T0 - 86_400 + u64::from(vid),
        retired: None,
        url: None,
    }
}

fn user(uid: &str, country: &str, battles: u64, hidden: bool, created: u64) -> User {
    User {
        uid: uid.to_string(),
        login: format!("login-{uid}"),
        name: String::new(),
        country: country.to_string(),
        github_id: uid[1..].parse().unwrap(),
        github_created: None,
        admin: false,
        trusted: false,
        hidden,
        bots: vec![],
        battles,
        last_post: None,
        created,
        reputation: Reputation {
            rep: 60,
            state: UserState::New,
            ..Reputation::default()
        },
    }
}

/// A 1v1 record: the stronger (higher vid) bot scores more.
fn battle(a: u32, b: u32, noise: f32) -> Record {
    let strength = |v: u32| if v > 50 { 25.0 } else { v as f32 };
    let pct = (50.0 + (strength(a) - strength(b)) * 0.8 + noise).clamp(5.0, 95.0);
    let firsts = (35.0 * pct / 100.0).round() as u16;
    Record {
        division: Division::OneVsOne,
        weight: 1,
        uid: String::new(),
        contestants: vec![
            Entry {
                vid: a,
                score: pct * 40.0,
                firsts,
            },
            Entry {
                vid: b,
                score: (100.0 - pct) * 40.0,
                firsts: 35 - firsts,
            },
        ],
        failed: None,
        trusted: true,
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % u64::from(n)) as u32
    }
}

/// Every record put into the log, with its poster.
struct Posted {
    records: Vec<(String, Record)>,
}

fn post(
    store: &dyn Store,
    posted: &mut Posted,
    hour: u32,
    k: u32,
    uid: &str,
    records: Vec<Record>,
    held: bool,
) {
    let start = rumble_core::batch::hour_start(hour).unwrap();
    let mut bytes = Vec::new();
    for r in &records {
        r.encode(&mut bytes);
    }
    let seeds: Vec<i64> = (0..records.len())
        .map(|i| i64::from(hour) * 1000 + i64::from(k) * 100 + i as i64)
        .collect();
    store
        .append_log(&LogItem {
            engine: 1,
            hour,
            sort: format!("{:013}#b-{uid}-{k}", (start + u64::from(k)) * 1000),
            uid: uid.to_string(),
            records: bytes,
            battles: records.len() as u32,
            seeds,
            held,
            ttl: start + LOG_TTL,
        })
        .unwrap();
    if !held {
        posted
            .records
            .extend(records.into_iter().map(|r| (uid.to_string(), r)));
    }
}

fn setup(world: &World) -> Posted {
    let store = world.store.as_ref();
    for vid in 1..=BOTS {
        assert_eq!(store.next_vid().unwrap(), vid);
        store
            .put_version(&version(vid, vid, "1.0", VersionState::Active))
            .unwrap();
    }
    // syn.Bot1 2.0 has finished its probation battles; syn.Bot2 2.0 has none yet.
    assert_eq!(store.next_vid().unwrap(), 51);
    store
        .put_version(&version(51, 1, "2.0", VersionState::Probation))
        .unwrap();
    assert_eq!(store.next_vid().unwrap(), 52);
    store
        .put_version(&version(52, 2, "2.0", VersionState::Probation))
        .unwrap();
    for bot in 1..=BOTS {
        let versions = match bot {
            1 => vec![51, 1],
            2 => vec![52, 2],
            _ => vec![bot],
        };
        store
            .put_bot(&BotItem {
                id: format!("syn.Bot{bot}"),
                author: format!("Author {bot}"),
                owner: (bot == 3).then(|| "g101".to_string()),
                homepage: Some(format!("https://example.org/{bot}")),
                description: Some("a synthetic bot".into()),
                divisions: vec![Division::OneVsOne],
                versions,
                current: Some(bot),
                state: BotState::Active,
                created: T0 - 86_400,
                source: BotSource::Import,
            })
            .unwrap();
    }
    for u in [
        user("g101", "NL", 400, false, T0 - 30 * 86_400),
        user("g102", "", 180, false, T0 - 6 * 3600),
        user("g103", "be", 50, true, T0 - 30 * 86_400),
        user("g104", "de", 20, false, T0 - 30 * 86_400),
    ] {
        store.create_user(&u).unwrap();
    }
    let quarantined = Reputation {
        state: UserState::Quarantined,
        ..Reputation::default()
    };
    store.set_reputation("g104", 0, &quarantined).unwrap();

    let mut posted = Posted {
        records: Vec::new(),
    };
    let mut rng = Lcg(42);
    let pair = |rng: &mut Lcg| loop {
        let (a, b) = (1 + rng.next(BOTS), 1 + rng.next(BOTS));
        if a != b {
            return (a, b);
        }
    };
    for &hour in &HOURS {
        let mut k = 0;
        for (uid, items) in [("g101", 6), ("g102", 3)] {
            for _ in 0..items {
                let records = (0..20)
                    .map(|_| {
                        let (a, b) = pair(&mut rng);
                        battle(a, b, rng.next(9) as f32 - 4.0)
                    })
                    .collect();
                post(store, &mut posted, hour, k, uid, records, false);
                k += 1;
            }
        }
        // A quarantined user's battles are held: never folded.
        let held = (0..5).map(|_| battle(3, 4, 0.0)).collect();
        post(store, &mut posted, hour, k, "g104", held, true);
        k += 1;
        if hour == HOURS[1] {
            // syn.Bot1 2.0's ten probation battles (a verifier's).
            let records = (0..10).map(|i| battle(51, 20 + i, 0.0)).collect();
            post(store, &mut posted, hour, k, "g101", records, false);
        }
    }
    posted
}

/// The pairing state the folding should produce from `records` (the retired and removed
/// versions dropped).
fn expected_state(records: &[(String, Record)], dropped: &[u32]) -> State {
    let mut state = State::new(1, Division::OneVsOne, 0);
    for (uid, record) in records {
        state.fold_from(record, 0, uid);
    }
    state.drop_versions(&dropped.iter().copied().collect());
    state
}

fn assert_same_pairings(actual: &State, expected: &State) {
    assert_eq!(actual.battles, expected.battles, "battles");
    let a = actual.sorted_pairings();
    let e = expected.sorted_pairings();
    assert_eq!(a.len(), e.len(), "pairings");
    for ((ka, pa), (ke, pe)) in a.iter().zip(&e) {
        assert_eq!((ka, pa.n), (ke, pe.n));
        assert!((pa.s - pe.s).abs() < 1e-6, "{ka:?}: {} vs {}", pa.s, pe.s);
    }
}

/// Writes the site's data files under `dir` (as /data/v1) and type-checks them with
/// `web/scripts/check-rumble-data.mjs`, if Node and the web dependencies are there.
fn check_with_node(world: &World, keys: &[String], dir: &Path) {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
    if !web.join("node_modules/typescript").exists() {
        eprintln!("skipping the types.ts check: web/node_modules has no typescript");
        return;
    }
    let _ = std::fs::remove_dir_all(dir);
    for key in keys {
        let path = dir.join(key.strip_prefix("data/v1/").unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, world.get(Bucket::Site, key).unwrap()).unwrap();
    }
    let output = match std::process::Command::new("node")
        .arg(web.join("scripts/check-rumble-data.mjs"))
        .arg(dir)
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            eprintln!("skipping the types.ts check: no node ({e})");
            return;
        }
    };
    assert!(
        output.status.success(),
        "types.ts check failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stdout).trim());
}

pub fn run(world: &World, data_dir: &Path) {
    let store = world.store.as_ref();
    let posted = setup(world);
    // Counted records per hour (g101 and g102), to build the expected states.
    let per_hour: usize = 9 * 20;
    let first_two: Vec<(String, Record)> = posted.records[..2 * per_hour + 10].to_vec();

    // ---- plan at 12:05: hours 09 and 10 are folded, 11 is not over long enough. ----
    let report = world.plan();
    assert_eq!(report.hours.first(), Some(&2026102311));
    assert_eq!(report.hours.last(), Some(&2026102410));
    let division = &report.divisions[0];
    assert_eq!(division.done, 2026102410);
    assert_eq!(division.last_hour_battles, 190);
    assert_eq!(division.hourly, 285, "1.5 × the last hour's battles");
    let state = world.state();
    assert_same_pairings(&state, &expected_state(&first_two, &[]));
    let plan = Plan::from_bytes(
        &world
            .get(Bucket::State, "state/e1/1v1/plan.json.gz")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(plan.bots.len(), 52, "50 active, 2 on probation");
    assert_eq!(plan.probation, vec![51, 52]);
    assert_eq!(
        plan.pairs.iter().map(|p| p.2).sum::<u32>(),
        plan.hourly,
        "quotas sum to the target"
    );
    assert!(plan.top.is_empty(), "no snapshot yet");
    // syn.Bot1 2.0 passed probation: active, the bot's current, 1.0 retired.
    assert_eq!(
        store.get_version(51).unwrap().unwrap().state,
        VersionState::Active
    );
    let old = store.get_version(1).unwrap().unwrap();
    assert_eq!((old.state, old.retired), (VersionState::Retired, Some(T0)));
    assert_eq!(
        store.get_bot("syn.Bot1").unwrap().unwrap().current,
        Some(51)
    );
    // syn.Bot2 2.0 got its ten probation battles in the queue.
    let queue = store.query_verify(1, None, 100).unwrap();
    assert_eq!(queue.len(), 10);
    assert!(queue.iter().all(|i| i.kind == VerifyKind::Probation
        && i.bots.len() == 2
        && i.bots.contains(&52)
        && !i.bots.contains(&2)));
    assert_eq!(report.probation_items, 10);
    // The queue isn't filled again on the next run.
    world.set(T0 + 120);
    assert_eq!(world.plan().probation_items, 0);
    assert_eq!(store.query_verify(1, None, 100).unwrap().len(), 10);

    // ---- publish at 12:08. ----
    world.set(T0 + 180);
    let report = world.publish();
    assert!(report.plan.hours.is_empty());
    let index: RumbleIndex = world.json("data/v1/rumbles.json");
    let ids: Vec<&str> = index.rumbles.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["1v1", "1v1-mini", "1v1-micro", "1v1-nano"]);
    assert_eq!(index.rumbles[0].bots, 50);
    let rankings: Rankings = world.json("data/v1/rankings/1v1.json");
    assert_eq!(rankings.bots.len(), 50);
    assert_eq!(rankings.bots[0].rank, 1);
    assert!(
        rankings.bots[0].id == "syn.Bot50" || rankings.bots[0].id == "syn.Bot49",
        "the strongest bots lead: {}",
        rankings.bots[0].id
    );
    let one: Vec<&str> = rankings
        .bots
        .iter()
        .filter(|r| r.id == "syn.Bot1")
        .map(|r| r.version.as_str())
        .collect();
    assert_eq!(one, vec!["2.0"], "the new version is ranked");
    let nano: Rankings = world.json("data/v1/rankings/1v1-nano.json");
    assert!(nano.bots.len() < 50 && !nano.bots.is_empty());
    let bot3: BotDetails = world.json("data/v1/bots/syn.Bot3.json");
    assert_eq!(bot3.author.github.as_deref(), Some("login-g101"));
    assert_eq!(bot3.author.country.as_deref(), Some("nl"));
    assert_eq!(bot3.rumbles["1v1"].history.len(), 1);
    let bot1: BotDetails = world.json("data/v1/bots/syn.Bot1.json");
    let versions: Vec<&str> = bot1.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions, vec!["2.0", "1.0"]);
    assert!(bot1.versions[1].retired_at.is_some());
    let contributors: Contributors = world.json("data/v1/contributors.json");
    let logins: Vec<&str> = contributors
        .contributors
        .iter()
        .map(|c| c.login.as_str())
        .collect();
    assert_eq!(
        logins,
        vec!["login-g101", "login-g102", "login-g104"],
        "the users that posted"
    );
    assert_eq!(contributors.contributors[0].tier, "verifier");
    assert_eq!(contributors.contributors[2].tier, "quarantined");
    assert_eq!(report.bot_files_written, 50);
    assert!(
        world
            .get(Bucket::State, "state/e1/snapshots/1v1/2026-10-24.json")
            .is_some()
    );
    assert!(world.cdn.paths().contains(&"/data/v1/*".to_string()));
    let data_keys = world.objects.list(Bucket::Site, "data/v1/").unwrap();
    check_with_node(world, &data_keys, data_dir);
    // A second run the same day changes no bot file: none is written.
    world.set(T0 + 190);
    let again = world.publish();
    assert_eq!(
        (again.bot_files_written, again.bot_files_unchanged),
        (0, 50)
    );
    // The plan now has `top` from the snapshot.
    world.set(T0 + 200);
    world.plan();
    let plan = Plan::from_bytes(
        &world
            .get(Bucket::State, "state/e1/1v1/plan.json.gz")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(plan.top.len(), 50, "50 bots, all in the top 50");

    // ---- a crash after the plan file, before the state: the rerun folds hour 11 once. ----
    world.set(T0 + 3600);
    let before = world
        .get(Bucket::State, &state_key(1, Division::OneVsOne))
        .unwrap();
    let report = world.plan();
    assert_eq!(report.hours, vec![2026102411]);
    let once = world.state();
    world
        .objects
        .put(
            Bucket::State,
            &state_key(1, Division::OneVsOne),
            &before,
            "application/octet-stream",
            None,
        )
        .unwrap();
    assert_eq!(world.plan().hours, vec![2026102411]);
    let rerun = world.state();
    assert_eq!(rerun.done, 2026102411);
    assert_same_pairings(&rerun, &once);
    assert!(world.plan().hours.is_empty(), "nothing left to fold");
    assert_same_pairings(&world.state(), &once);
    assert_same_pairings(&once, &expected_state(&posted.records, &[1]));

    // Recordings: of the retired syn.Bot1 1.0, of syn.Bot7 (taken down below), of others.
    let v7 = store.get_version(7).unwrap().unwrap();
    let v9 = store.get_version(9).unwrap().unwrap();
    let v1 = store.get_version(1).unwrap().unwrap();
    let dir = |v: &VersionItem| format!("{}_{}", v.bot, &v.sha256[..16]);
    let rec7 = format!("rec/e1/1v1/{}/{}/a-0.track", dir(&v7), dir(&v9));
    let rec1 = format!("rec/e1/1v1/{}/{}/b-1.track", dir(&v1), dir(&v9));
    let rec_other = format!("rec/e1/1v1/{}/{}/a-0.track", dir(&v9), dir(&v9));
    for key in [&rec7, &rec1, &rec_other] {
        world
            .objects
            .put(Bucket::Jars, key, b"track", TRACK_TYPE, Some(IMMUTABLE))
            .unwrap();
    }

    // ---- a ban: corrections for every counted record of g102, over several runs. ----
    world.set(T0 + 3660);
    bots::update_user(
        store,
        &UserUpdateRequest {
            uid: "g102".into(),
            state: Some("ban".into()),
            ..UserUpdateRequest::default()
        },
        world.now_ms(),
    )
    .unwrap();
    world.tick.store(50, Ordering::SeqCst);
    let mut runs = 0;
    let ban = loop {
        runs += 1;
        let report = world.cleanup(150);
        let ban = report.bans.into_iter().find(|b| b.uid == "g102").unwrap();
        if ban.finished || runs > 40 {
            break ban;
        }
    };
    world.tick.store(0, Ordering::SeqCst);
    assert!(ban.finished && runs > 1, "{runs} runs");
    // The first of those runs cleaned the retired version: its recording is gone, not
    // invalidated (it is no secret).
    assert!(!world.objects.exists(Bucket::Jars, &rec1).unwrap());
    assert!(world.objects.exists(Bucket::Jars, &rec_other).unwrap());
    assert!(world.cdn.paths().iter().all(|p| !p.starts_with("/rec/")));
    assert_eq!(ban.corrected, 3 * 60);
    let correction_hour = hour_key(world.now_ms() / 1000);
    let corrections: Vec<LogItem> = store
        .query_log(1, correction_hour)
        .unwrap()
        .into_iter()
        .filter(|i| i.uid == SYSTEM_UID)
        .collect();
    let sorts: HashSet<&str> = corrections.iter().map(|i| i.sort.as_str()).collect();
    assert_eq!(sorts.len(), corrections.len());
    assert_eq!(
        corrections.iter().map(|i| i.battles).sum::<u32>(),
        180,
        "written once"
    );
    // Another run does nothing more.
    assert!(world.cleanup(u64::MAX).bans.is_empty());
    // Folded (once their hour is over), the corrections remove g102's battles.
    world.set(T0 + 3 * 3600);
    world.plan();
    let without: Vec<(String, Record)> = posted
        .records
        .iter()
        .filter(|(uid, _)| uid != "g102")
        .cloned()
        .collect();
    assert_same_pairings(&world.state(), &expected_state(&without, &[1]));

    // ---- a takedown: jar, recordings and slot item go, CloudFront forgets them. ----
    let jar = format!("jars/{}.jar", v7.sha256);
    world
        .objects
        .put(Bucket::Jars, &jar, b"PK jar", JAR_TYPE, Some(IMMUTABLE))
        .unwrap();
    let slot = "1v1.7.9.a.0".to_string();
    store
        .update_slots(
            1,
            Division::OneVsOne,
            "7#9",
            &[(slot.clone(), None)],
            &[(
                slot,
                SlotEntry {
                    status: SlotStatus::Filled,
                    batch: "b-x".into(),
                    index: 0,
                    round: 0,
                    sha256_b64: "x".into(),
                    size: 5,
                    claim_until: 0,
                    seed: 1,
                },
            )],
            false,
        )
        .unwrap();
    bots::remove(store, "syn.Bot7", "the author asked", world.now_ms()).unwrap();
    let report = world.cleanup(u64::MAX);
    assert_eq!(report.jars_deleted, 1);
    assert_eq!(report.recordings_deleted, 1);
    assert_eq!(report.slot_items_deleted, 1);
    assert_eq!(report.versions_cleaned, vec![7]);
    assert!(!world.objects.exists(Bucket::Jars, &jar).unwrap());
    assert!(!world.objects.exists(Bucket::Jars, &rec7).unwrap());
    assert!(world.objects.exists(Bucket::Jars, &rec_other).unwrap());
    assert!(
        store
            .get_slots(1, Division::OneVsOne, "7#9")
            .unwrap()
            .is_none()
    );
    let paths = world.cdn.paths();
    assert!(paths.contains(&format!("/{jar}")), "{paths:?}");
    assert!(paths.contains(&format!("/{rec7}")), "{paths:?}");
    assert!(!paths.contains(&format!("/{rec1}")), "retired: not secret");
    // Done once: the next run has nothing to clean.
    let again = world.cleanup(u64::MAX);
    assert!(again.versions_cleaned.is_empty() && again.invalidated.is_empty());
    // The next publish drops the bot and its file.
    world.set(T0 + 3 * 3600 + 600);
    let report = world.publish();
    assert_eq!(report.bot_files_deleted, 1);
    assert!(
        world
            .get(Bucket::Site, "data/v1/bots/syn.Bot7.json")
            .is_none()
    );
    let rankings: Rankings = world.json("data/v1/rankings/1v1.json");
    assert!(rankings.bots.iter().all(|r| r.id != "syn.Bot7"));
    let contributors: Contributors = world.json("data/v1/contributors.json");
    assert!(
        contributors
            .contributors
            .iter()
            .all(|c| c.login != "login-g102"),
        "banned"
    );
}
