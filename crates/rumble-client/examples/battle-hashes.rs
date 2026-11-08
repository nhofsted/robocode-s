// SPDX-License-Identifier: EPL-2.0

//! Runs the duels and melees of a difftest corpus plan as rumble battles and prints each
//! battle's hash, the one rumble verification compares: `cargo run --release -p
//! robocode-s-rumble-client --example battle-hashes -- PLAN.json JARS_DIR [ROUNDS] [ONLY]`.
//! Two robot hosts that play every battle alike print the same lines, so this checks a
//! change of the robot host (a new classic version, ADR 0027) against the old one: run it
//! once with each `ROBOCODES_HOST_LIB` and compare the output. `ONLY` keeps the battles whose
//! name contains it (`duel`). A robot `a.B 1.0` is `JARS_DIR/a.B_1.0.jar`; one without a
//! version is a sample robot (`SAMPLES_JAR`). Needs Java and the built robot host, as the
//! ignored battle tests do; `ROBOCODES_JAVA`, `ROBOCODES_AGENT_JAR` and `ROBOCODES_DATA` as
//! for `robocode-s`.

use robocode_s_runner::local::Environment;
use rumble_client::battle::{Options, Runner};
use rumble_core::api::{BattleKind, Rules, WorkBattle, WorkBot};
use rumble_core::ids::Division;
use std::path::{Path, PathBuf};

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: battle-hashes PLAN.json JARS_DIR [ROUNDS] [ONLY]";
    let plan_file = args.next().expect(usage);
    let jars_dir = PathBuf::from(args.next().expect(usage));
    let max_rounds: Option<u32> = args.next().map(|r| r.parse().expect("rounds"));
    let only = args.next();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let env = Environment::from_env().rooted_at(&root);
    let samples: PathBuf = std::env::var_os("SAMPLES_JAR").map_or_else(
        || root.join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar"),
        PathBuf::from,
    );
    let runner = Runner::new(Options::new(env)).expect("runner");
    let plan: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&plan_file).expect("plan"))
            .expect("plan JSON");
    for (index, entry) in plan
        .as_array()
        .expect("a plan is a list")
        .iter()
        .enumerate()
    {
        let name = entry["name"].as_str().unwrap_or_default();
        let melee = name.contains("melee");
        if !(name.contains("duel") || melee) || only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let robots: Vec<&str> = entry["robots"]
            .as_array()
            .expect("robots")
            .iter()
            .map(|r| r.as_str().expect("robot"))
            .collect();
        let (bots, jars): (Vec<WorkBot>, Vec<PathBuf>) = robots
            .iter()
            .map(|robot| {
                let (id, version) = robot.split_once(' ').unwrap_or((robot, ""));
                let jar = if version.is_empty() {
                    samples.clone()
                } else {
                    jars_dir.join(format!("{id}_{version}.jar"))
                };
                let bot = WorkBot {
                    id: id.into(),
                    version: version.into(),
                    vid: 1,
                    sha256: "0".repeat(64),
                    size: 1,
                    url: String::new(),
                    data: None,
                };
                (bot, jar)
            })
            .unzip();
        let rounds = entry["rounds"].as_u64().unwrap_or(10) as u32;
        let battle = WorkBattle {
            index: index as u32,
            kind: BattleKind::Normal,
            division: if melee {
                Division::Melee
            } else {
                Division::OneVsOne
            },
            seed: entry["seed"].as_i64().expect("seed"),
            rules: Rules {
                rounds: max_rounds.map_or(rounds, |m| rounds.min(m)),
                width: if melee { 1000 } else { 800 },
                height: if melee { 1000 } else { 600 },
                gun_cooling_rate: 0.1,
                inactivity_time: 450,
                sentry_border: 100,
                hide_enemy_names: false,
            },
            bots,
            twice: false,
        };
        let outcome = runner.run(&battle, &jars);
        // Robots that ran out of memory (#178): that battle may differ between machines.
        let out_of_memory: Vec<&str> = robots
            .iter()
            .zip(outcome.result.robots.iter().flatten())
            .filter(|(_, r)| r.out_of_memory)
            .map(|(name, _)| *name)
            .collect();
        println!(
            "{name} {:?} {} {}{}",
            outcome.result.status,
            outcome.result.battle_hash.as_deref().unwrap_or("-"),
            outcome.why.as_deref().unwrap_or(""),
            if out_of_memory.is_empty() {
                String::new()
            } else {
                format!(" out of memory: {}", out_of_memory.join(", "))
            }
        );
    }
}
