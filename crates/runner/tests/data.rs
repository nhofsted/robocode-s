// SPDX-License-Identifier: EPL-2.0

//! Robots' data files and what a battle leaves on disk (design §6.5, §6.7, §6.8, #86): a
//! robot's files (`sample.SittingDuck` counts its rounds and battles in one) are kept
//! between battles, and the battle's folder and the warm hosts' homes are gone when it is
//! over. Needs Java and the built robot host and sample robots (`./gradlew
//! :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar`),
//! so it is ignored by default: run it with `cargo test -p robocode-s-runner --test data
//! -- --ignored`.

use robocode_s_engine::BattleRules;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, prepare_with, run_local_battle};
use robocode_s_runner::pool::Pool;
use robocode_s_server::ServerConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// The file a robot's data folder holds, wherever below it classic put it.
fn count_file(data: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(data).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "count.dat") {
            return Some(path);
        }
        if let Some(found) = count_file(&path) {
            return Some(found);
        }
    }
    None
}

/// `rounds` and `battles` of `sample.SittingDuck`'s count file.
fn counted(data: &Path) -> Vec<String> {
    let file = count_file(&data.join("sample.SittingDuck")).expect("a data file");
    std::fs::read_to_string(file)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |d| d.count())
}

/// Two battles of the same robot, one cold and one with a warm host: both leave no
/// folders, and the second sees what the first wrote.
fn data_files_stay(sandbox: Sandbox) {
    if let Some(why) = sandbox.unavailable() {
        eprintln!("{sandbox} skipped: {why}");
        return;
    }
    sandbox.prepare().unwrap();
    let env = environment(sandbox);
    let scratch = std::env::temp_dir().join(format!(
        "robocodes-data-test-{sandbox}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    let (data, pool_dir) = (scratch.join("robot-data"), scratch.join("pool"));
    let robots = ["sample.SittingDuck".to_string(), "sample.Fire".to_string()];
    let config = || ServerConfig {
        rules: BattleRules {
            num_rounds: 2,
            ..Default::default()
        },
        seed: 5,
        deterministic: true,
        ..Default::default()
    };
    for (battle_number, warm) in [(1, false), (2, true)] {
        let pool = warm.then(|| Arc::new(Pool::new(pool_dir.clone())));
        let (host, mut battle) =
            prepare_with(&env, &robots, config(), 256, pool.as_ref().map(|p| (p, 2))).unwrap();
        battle.data = Some(data.clone());
        let dir = battle.dir.clone();
        run_local_battle(&host, &battle, None, |_| {}).unwrap();
        assert!(!dir.exists(), "{sandbox}: the battle's folder is gone");
        assert_eq!(
            counted(&data),
            [(2 * battle_number).to_string(), battle_number.to_string()],
            "{sandbox}: battle {battle_number}"
        );
        if let Some(pool) = &pool {
            assert!(pool.assigned() > 0, "{sandbox}: a warm host was used");
            pool.no_refill();
        }
        // The battle holds the pool too.
        drop(battle);
        drop(pool);
        assert_eq!(files_in(&pool_dir), 0, "{sandbox}: no home is left");
    }
    // No data folder: the robot starts afresh and keeps nothing.
    let (host, mut battle) = prepare_with(&env, &robots, config(), 256, None).unwrap();
    battle.data = None;
    run_local_battle(&host, &battle, None, |_| {}).unwrap();
    assert_eq!(counted(&data)[1], "2", "{sandbox}: fresh data kept nothing");
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
#[ignore = "needs Java and the built robots"]
fn data_files_stay_without_a_sandbox() {
    data_files_stay(Sandbox::None);
}

#[test]
#[ignore = "needs Java and the built robots"]
fn data_files_stay_in_the_os_sandbox() {
    data_files_stay(Sandbox::Os);
}

#[test]
#[ignore = "needs Java, the built robots and Docker"]
fn data_files_stay_in_a_container() {
    data_files_stay(Sandbox::Docker);
}
