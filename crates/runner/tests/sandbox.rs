// SPDX-License-Identifier: EPL-2.0

//! A deterministic battle of real robot JVMs in a sandbox (the platform's `os` one, or
//! `docker`) has the results of the same battle without one: the sandbox changes nothing a
//! robot does. The `docker` battle also leaves no container behind. Needs Java and the built
//! robot host and sample robots (`./gradlew :clients:java-compat:installDist
//! :clients:java-determinism:jar :robots:samples:jar`), and Docker for its test (the image is
//! pulled the first time), so they are ignored by default: run them with `cargo test -p
//! robocode-s-runner -- --ignored` (`ROBOCODES_JAVA` names the JDK to use; on Windows the
//! JRE, the robot host and the robots must be on NTFS, `ROBOCODES_DATA` and the others).

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::Path;
use std::process::Command;

/// The results of a 3-round deterministic battle of four sample robots in `sandbox`.
fn fight(sandbox: Sandbox) -> Vec<v1::Results> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let env = Environment {
        sandbox,
        ..Environment::from_env().rooted_at(&root)
    };
    let robots = [
        "sample.Fire",
        "sample.Walls",
        "sample.SpinBot",
        "sample.Tracker",
    ]
    .map(String::from);
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
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_battle_in_the_os_sandbox_is_the_battle_without_one() {
    if let Some(why) = Sandbox::Os.unavailable() {
        eprintln!("skipped: {why}");
        return;
    }
    assert_eq!(fight(Sandbox::Os), fight(Sandbox::None));
}

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
