// SPDX-License-Identifier: EPL-2.0

//! A deterministic battle of real robot JVMs in the `docker` sandbox has the results of the
//! same battle in the `os` sandbox and without one, and leaves no container behind. Needs
//! Docker (the image is pulled the first time), Java and the built robot host and sample
//! robots (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-runner -- --ignored`.

use robocode_s_engine::BattleRules;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::Path;
use std::process::Command;

/// The runner's containers, running or not.
fn containers() -> String {
    let output = Command::new("docker")
        .args(["ps", "--all", "--quiet", "--filter", "label=robocode-s=1"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
#[ignore = "needs Docker, Java and the built robots"]
fn a_battle_in_docker_is_the_battle_without_it() {
    if let Some(why) = Sandbox::Docker.unavailable() {
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
    let docker = fight(Sandbox::Docker);
    assert_eq!(docker, fight(Sandbox::None));
    if Sandbox::Os.unavailable().is_none() {
        assert_eq!(docker, fight(Sandbox::Os));
    }
    // `--rm` removes a container as it ends: give the engine a moment.
    for _ in 0..100 {
        if containers().is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("containers left behind: {}", containers());
}
