// SPDX-License-Identifier: EPL-2.0

//! A deterministic battle of real robot JVMs in the platform's `os` sandbox has the
//! results of the same battle without one: the sandbox changes nothing a robot does.
//! Needs Java and the built robot host and sample robots
//! (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-runner -- --ignored` (`ROBOCODES_JAVA` names the JDK to use; on Windows the
//! JRE, the robot host and the robots must be on NTFS, `ROBOCODES_DATA` and the others).

use robocode_s_engine::BattleRules;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::Path;

#[test]
#[ignore = "needs Java and the built robots"]
fn a_battle_in_the_os_sandbox_is_the_battle_without_one() {
    if let Some(why) = Sandbox::Os.unavailable() {
        eprintln!("skipped: {why}");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    let robots = [
        "sample.Fire",
        "sample.Walls",
        "sample.SpinBot",
        "sample.Tracker",
    ]
    .map(String::from);
    let fight = |sandbox| {
        let env = Environment {
            sandbox,
            ..env.clone()
        };
        let config = ServerConfig {
            rules: BattleRules {
                num_rounds: 3,
                ..Default::default()
            },
            seed: 11,
            deterministic: true,
            ..Default::default()
        };
        let (host, battle) = prepare(&env, &robots, config, 256).unwrap();
        let local = run_local_battle(&host, &battle, None, |_| {}).unwrap();
        let _ = std::fs::remove_dir_all(&battle.dir);
        local.outcome.results
    };
    assert_eq!(fight(Sandbox::Os), fight(Sandbox::None));
}
