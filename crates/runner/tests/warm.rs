// SPDX-License-Identifier: EPL-2.0

//! Warm hosts (design §6.2, #82): robots started from a pool play a deterministic battle
//! exactly as cold ones do, snapshot for snapshot; and what starting them ahead saves.
//! Needs Java and the built robot host and sample robots (`./gradlew
//! :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar`), so
//! it is ignored by default: run it with `cargo test -p robocode-s-runner --test warm --
//! --ignored` (`--nocapture` shows the timings; `WARM_SANDBOXES=none,os,docker` picks the
//! sandboxes the timing test runs in).

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::server_message::Msg;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, prepare, prepare_with, run_local_battle};
use robocode_s_runner::pool::Pool;
use robocode_s_server::ServerConfig;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn environment(sandbox: Sandbox) -> Environment {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    Environment { sandbox, ..env }
}

fn config(rounds: i32, seed: i64, deterministic: bool) -> ServerConfig {
    ServerConfig {
        rules: BattleRules {
            num_rounds: rounds,
            ..Default::default()
        },
        seed,
        deterministic,
        ..Default::default()
    }
}

fn names(robots: &[&str]) -> Vec<String> {
    robots.iter().map(|r| r.to_string()).collect()
}

const FOUR: [&str; 4] = [
    "sample.Fire",
    "sample.Walls",
    "sample.SpinBot",
    "sample.Tracker",
];

/// What a battle came to: its results and a hash of every turn's snapshot, and when its
/// first turn came and it ended.
struct Fought {
    results: String,
    turns: usize,
    hash: u64,
    first_turn: Duration,
    total: Duration,
}

/// Prepares and fights a battle of `robots`, with hosts from `pool` (warmed during the
/// description, as the CLI does) or cold.
fn fight(env: &Environment, robots: &[String], rounds: i32, pool: Option<&Arc<Pool>>) -> Fought {
    let start = Instant::now();
    let warm = pool.map(|p| (p, robots.len()));
    let (host, battle) = prepare_with(env, robots, config(rounds, 11, true), 256, warm).unwrap();
    let mut fought = play(&host, &battle, start);
    let _ = std::fs::remove_dir_all(&battle.dir);
    fought.total = start.elapsed();
    fought
}

fn play(
    host: &robocode_s_runner::JavaHost,
    battle: &robocode_s_runner::local::LocalBattle,
    start: Instant,
) -> Fought {
    let mut hasher = DefaultHasher::new();
    let (mut turns, mut first_turn) = (0, None);
    let local = run_local_battle(host, battle, None, |msg| {
        if let Msg::TurnSnapshot(s) = msg {
            first_turn.get_or_insert_with(|| start.elapsed());
            turns += 1;
            format!("{s:?}").hash(&mut hasher);
        }
    })
    .unwrap();
    Fought {
        results: format!("{:?}", local.outcome.results),
        turns,
        hash: hasher.finish(),
        first_turn: first_turn.unwrap(),
        total: start.elapsed(),
    }
}

fn pool(env: &Environment, name: &str) -> Arc<Pool> {
    Arc::new(Pool::new(env.data.join(format!(
        "pool-test-{name}-{}-{}",
        env.sandbox,
        std::process::id()
    ))))
}

/// A battle with warm hosts is the battle with cold ones: the same snapshots in every
/// turn, so also what the robots saw and did.
fn warm_is_cold(sandbox: Sandbox) {
    if let Some(why) = sandbox.unavailable() {
        eprintln!("{sandbox} skipped: {why}");
        return;
    }
    sandbox.prepare().unwrap();
    let env = environment(sandbox);
    let robots = names(&FOUR);
    let cold = fight(&env, &robots, 3, None);
    let pool = pool(&env, "same");
    let warm = fight(&env, &robots, 3, Some(&pool));
    assert_eq!(warm.results, cold.results, "sandbox {sandbox}");
    assert_eq!(
        (warm.turns, warm.hash),
        (cold.turns, cold.hash),
        "{sandbox}"
    );
    // The pool refilled while the battle ran: a second battle takes those hosts, and a
    // robot host that was started warm and used once is not used again.
    assert_eq!(
        pool.assigned(),
        4,
        "{sandbox}: robots started from the pool"
    );
    let again = fight(&env, &robots, 3, Some(&pool));
    assert_eq!(pool.assigned(), 8, "{sandbox}: the refilled hosts");
    assert_eq!(
        (again.turns, again.hash),
        (cold.turns, cold.hash),
        "{sandbox}"
    );
}

#[test]
#[ignore = "needs Java and the built robots"]
fn warm_hosts_play_the_battle_cold_ones_do_without_a_sandbox() {
    warm_is_cold(Sandbox::None);
}

#[test]
#[ignore = "needs Java and the built robots"]
fn warm_hosts_play_the_battle_cold_ones_do_in_the_os_sandbox() {
    warm_is_cold(Sandbox::Os);
}

#[test]
#[ignore = "needs Docker, Java and the built robots"]
fn warm_hosts_play_the_battle_cold_ones_do_in_docker() {
    warm_is_cold(Sandbox::Docker);
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_pool_for_other_settings_is_not_used() {
    let env = environment(Sandbox::None);
    let robots = names(&["sample.Walls", "sample.Fire"]);
    let pool = pool(&env, "other");
    // Hosts warmed for a classic-mode battle don't serve a deterministic one: they start
    // cold, and the battle is the same.
    let (host, battle) = prepare(&env, &robots, config(1, 3, true), 256).unwrap();
    pool.warm(&host, 256, false, 2);
    let cold = play(&host, &battle, Instant::now());
    let with_pool = {
        let battle = robocode_s_runner::local::LocalBattle {
            pool: Some(pool.clone()),
            ..battle.clone()
        };
        play(&host, &battle, Instant::now())
    };
    assert_eq!((with_pool.turns, with_pool.hash), (cold.turns, cold.hash));
    assert_eq!(pool.assigned(), 0, "hosts of another kind were taken");
    let _ = std::fs::remove_dir_all(&battle.dir);
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// How long battles take to start with and without a pool: the CLI's battle (describe,
/// then start) of four robots, and a rumble-like loop of ten short 1v1 battles of a known
/// description. Prints the medians.
#[test]
#[ignore = "a measurement: needs Java and the built robots"]
fn what_warm_hosts_save() {
    let sandboxes = std::env::var("WARM_SANDBOXES").unwrap_or_else(|_| "none,os".into());
    for sandbox in sandboxes.split(',') {
        let sandbox: Sandbox = sandbox.parse().unwrap();
        sandbox.prepare().unwrap();
        let env = environment(sandbox);
        let robots = names(&FOUR);
        // One to warm up the disk cache and the robot list.
        fight(&env, &robots, 3, None);
        let (mut cold, mut warm) = (Vec::new(), Vec::new());
        for _ in 0..5 {
            cold.push(fight(&env, &robots, 3, None));
            let pool = pool(&env, "cli");
            warm.push(fight(&env, &robots, 3, Some(&pool)));
        }
        let m = |f: &[Fought], g: fn(&Fought) -> Duration| median(f.iter().map(g).collect());
        println!(
            "{sandbox}: CLI 4 robots 3 rounds: first turn {:?} cold, {:?} warm; total {:?} cold, {:?} warm",
            m(&cold, |f| f.first_turn),
            m(&warm, |f| f.first_turn),
            m(&cold, |f| f.total),
            m(&warm, |f| f.total),
        );

        // A rumble: the same two robots, one round each time, from a known description.
        let pair = names(&["sample.Walls", "sample.Fire"]);
        let (host, battle) = prepare(&env, &pair, config(1, 1, true), 256).unwrap();
        let rumble = |pool: Option<&Arc<Pool>>| {
            let begin = Instant::now();
            let mut starts = Vec::new();
            for seed in 0..10 {
                let mut battle = battle.clone();
                battle.config.seed = seed;
                battle.pool = pool.cloned();
                let at = Instant::now();
                starts.push(play(&host, &battle, at).first_turn);
            }
            (begin.elapsed(), median(starts))
        };
        let (cold_total, cold_start) = rumble(None);
        let pool = pool(&env, "rumble");
        pool.warm(&host, 256, true, 2);
        // The first battle takes hosts that had no time to start: wait as a rumble would
        // not have to.
        std::thread::sleep(Duration::from_secs(3));
        let (warm_total, warm_start) = rumble(Some(&pool));
        println!(
            "{sandbox}: rumble of 10 1v1 battles: {cold_total:?} cold, {warm_total:?} warm; \
             battle start to first turn {cold_start:?} cold, {warm_start:?} warm"
        );
        let _ = std::fs::remove_dir_all(&battle.dir);
    }
}
