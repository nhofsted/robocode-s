// SPDX-License-Identifier: EPL-2.0

//! Rumble battles end to end (docs/rumble-backend.md §13.2, §13.4): the same seeded battle
//! gives the same hashes and track bytes, another seed doesn't, the per-round scores add up,
//! every turn is observed, tracks decode to the snapshots within their quantization, and bad
//! work items fail as the results API says. Needs Java and the built robot host and sample
//! robots (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-rumble-client --test battle -- --ignored` (`--nocapture` shows the timings).

use robocode_s_replay::{ANGLE_STEPS, ENERGY_SCALE, POS_SCALE, RobotState};
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::Environment;
use rumble_client::battle::{Options, Outcome, Runner};
use rumble_core::api::{BattleKind, BattleStatus, FailureReason, Rules, WorkBattle, WorkBot};
use rumble_core::ids::Division;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn environment(sandbox: Sandbox) -> Environment {
    let root = root();
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    Environment { sandbox, ..env }
}

fn samples_jar() -> PathBuf {
    std::path::absolute(root().join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar"))
        .unwrap()
}

fn bot(id: &str) -> WorkBot {
    WorkBot {
        id: id.into(),
        version: String::new(),
        vid: 1,
        sha256: "0".repeat(64),
        size: 1,
        url: String::new(),
        data: None,
    }
}

fn battle(seed: i64, bots: &[&str]) -> WorkBattle {
    WorkBattle {
        index: 3,
        kind: BattleKind::Normal,
        division: Division::OneVsOne,
        seed,
        rules: Rules {
            rounds: 3,
            width: 800,
            height: 600,
            gun_cooling_rate: 0.1,
            inactivity_time: 450,
            sentry_border: 100,
            hide_enemy_names: false,
        },
        bots: bots.iter().map(|b| bot(b)).collect(),
    }
}

fn runner() -> Runner {
    let sandbox = Sandbox::default();
    let mut options = Options::new(environment(sandbox));
    options.allow_unsandboxed = sandbox == Sandbox::None;
    options.keep_snapshots = true;
    Runner::new(options).unwrap()
}

fn run(runner: &Runner, battle: &WorkBattle) -> Outcome {
    let jars = vec![samples_jar(); battle.bots.len()];
    let started = Instant::now();
    let outcome = runner.run(battle, &jars);
    eprintln!(
        "battle seed {} ({:?}): {:?} in {:.2} s",
        battle.seed,
        battle.bots.iter().map(|b| &b.id).collect::<Vec<_>>(),
        outcome.result.status,
        started.elapsed().as_secs_f64()
    );
    outcome
}

fn ok(outcome: &Outcome) {
    assert_eq!(
        outcome.result.status,
        BattleStatus::Ok,
        "{:?} {:?}",
        outcome.why,
        outcome.result.failure
    );
}

/// Angles in units of `steps` per turn, the shorter way round.
fn angle_off(units: i64, radians: f64, steps: i64) -> f64 {
    let d = units as f64 * (2.0 * PI / steps as f64) - radians;
    (d + PI).rem_euclid(2.0 * PI) - PI
}

#[test]
#[ignore = "needs Java and the built robots"]
fn seeded_battles_hash_and_track_alike() {
    let runner = runner();
    runner.check_host().unwrap();
    let robots = ["sample.Walls", "sample.Crazy"];
    let a = run(&runner, &battle(42, &robots));
    let b = run(&runner, &battle(42, &robots));
    let c = run(&runner, &battle(43, &robots));
    for o in [&a, &b, &c] {
        ok(o);
    }
    let (ra, rb, rc) = (&a.result, &b.result, &c.result);
    assert_eq!(ra.index, 3);
    assert_eq!(ra.battle_hash, rb.battle_hash);
    assert_eq!(ra.rounds, rb.rounds, "round hashes, scores, tracks' claims");
    assert_eq!(ra.robots, rb.robots);
    assert_eq!(a.tracks, b.tracks, "track bytes");
    assert_ne!(ra.battle_hash, rc.battle_hash, "another seed");
    assert_ne!(
        ra.rounds.as_ref().unwrap()[0].hash,
        rc.rounds.as_ref().unwrap()[0].hash
    );

    let robots = ra.robots.as_ref().unwrap();
    let rounds = ra.rounds.as_ref().unwrap();
    assert_eq!(rounds.len(), 3);
    assert_eq!(a.tracks.len(), 3);
    assert_eq!(a.snapshots.len(), 3);
    for (r, robot) in robots.iter().enumerate() {
        let sum: f64 = rounds.iter().map(|round| round.scores[r]).sum();
        assert!(
            (sum - robot.score).abs() <= 1e-6 * robot.score.abs().max(1.0),
            "robot {r}: rounds sum to {sum}, total {}",
            robot.score
        );
        let won = rounds
            .iter()
            .filter(|round| round.winner == r as i32)
            .count();
        assert_eq!(won as u32, robot.firsts, "robot {r}'s firsts");
    }

    for ((round, bytes), snapshots) in rounds.iter().zip(&a.tracks).zip(&a.snapshots) {
        // Every turn is seen: the round start (turn 0), then one per turn.
        assert_eq!(snapshots.len() as u32, round.turns + 1);
        assert!(
            snapshots
                .iter()
                .enumerate()
                .all(|(t, s)| s.turn == t as i64)
        );
        let claim = round.track.as_ref().unwrap();
        assert_eq!(claim.size, bytes.len() as u64);
        assert_eq!(claim.sha256, rumble_client::http::sha256_hex(bytes));
        assert_eq!(round.hash.len(), 64);

        let track = robocode_s_replay::decode(bytes).unwrap();
        assert_eq!(track.engine_version, robocode_s_engine::ENGINE_VERSION);
        assert_eq!(track.frames.len(), snapshots.len());
        assert!(!track.bullets.is_empty(), "the robots fired");
        let mut dead = [false; 2];
        for (frame, snapshot) in track.frames.iter().zip(snapshots) {
            for (i, (f, s)) in frame.iter().zip(&snapshot.robots).enumerate() {
                if dead[i] {
                    assert_eq!(f.state, RobotState::Dead);
                    continue; // a dead robot stays as it died
                }
                dead[i] = f.state == RobotState::Dead;
                let half = |scale: i64| 0.5 / scale as f64 + 1e-9;
                assert!((f.x as f64 / POS_SCALE as f64 - s.x.unwrap()).abs() <= half(POS_SCALE));
                assert!((f.y as f64 / POS_SCALE as f64 - s.y.unwrap()).abs() <= half(POS_SCALE));
                assert!(
                    (f.energy as f64 / ENERGY_SCALE as f64 - s.energy.unwrap()).abs()
                        <= half(ENERGY_SCALE)
                );
                let step = PI / ANGLE_STEPS as f64 + 1e-9;
                for (units, radians) in [
                    (f.body, s.body_heading.unwrap()),
                    (f.gun, s.gun_heading.unwrap()),
                    (f.radar, s.radar_heading.unwrap()),
                ] {
                    assert!(angle_off(units, radians, ANGLE_STEPS).abs() <= step);
                }
            }
        }
    }
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_data_snapshot_is_not_run() {
    let runner = runner();
    let mut work = battle(1, &["sample.Walls", "sample.Crazy"]);
    work.bots[1].data = Some(serde_json::json!({"sha256": "00", "size": 1, "url": "/x"}));
    let outcome = run(&runner, &work);
    assert_eq!(outcome.result.status, BattleStatus::NotRun);
    assert!(outcome.result.battle_hash.is_none());
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_missing_robot_fails_to_load() {
    let runner = runner();
    let outcome = run(&runner, &battle(1, &["sample.Walls", "sample.NoSuchRobot"]));
    assert_eq!(outcome.result.status, BattleStatus::RobotFailed);
    let failure = outcome.result.failure.unwrap();
    assert_eq!(failure.robot, 1);
    assert_eq!(failure.reason, FailureReason::Load);
    assert!(failure.detail.unwrap().contains("not found"));
}
