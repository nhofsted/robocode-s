// SPDX-License-Identifier: EPL-2.0

//! Robots that start threads of their own that the scheduler doesn't run deterministically
//! (ADR 0025) are reported (ADR 0024); other robots aren't.
//! Needs Java and the built robot host, agent and robots (`./gradlew
//! :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
//! :robots:testbots:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-runner --test threads -- --ignored` (`ROBOCODES_JAVA` names the JDK to use).

use robocode_s_engine::BattleRules;
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::Path;

#[test]
#[ignore = "needs Java and the built robots"]
fn robots_that_start_threads_are_reported() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env().rooted_at(&root);
    env.jars
        .push(root.join("robots/testbots/build/libs/testbots-0.1.0-SNAPSHOT.jar"));
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            ..Default::default()
        },
        seed: 5,
        deterministic: true,
        ..Default::default()
    };
    let robots = [
        "sample.Fire".to_string(),
        "testbots.Threads".to_string(),
        "sample.Walls".to_string(),
        "testbots.Timers".to_string(),
        "testbots.Monitors".to_string(),
        "testbots.Virtual".to_string(),
    ];
    let (host, battle) = prepare(&env, &robots, config, 256).unwrap();
    let local = run_local_battle(&host, &battle, None, |_| {}).unwrap();
    // By robot index: the scheduler runs the threads of testbots.Threads, Timers (a Timer's) and
    // Monitors deterministically; only testbots.Virtual's virtual thread is reported.
    assert_eq!(
        local.outcome.threads,
        [false, false, false, false, false, true]
    );
}

/// Classic 1.11.2's robot side punishes a robot with more than 5 threads besides its run
/// thread (`RobotThreadManager.checkThreadCount`, every turn). testbots.Pool makes a pool of
/// two threads each round and never shuts it down, so it has 6 in round 3. The punished robot
/// must end its round as classic's does (its next `execute` throws `DeathException`), not
/// hang until the hard limit, and the battle must come out the same every time.
#[test]
#[ignore = "needs Java and the built robots"]
fn a_robot_with_too_many_threads_is_punished_reproducibly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env().rooted_at(&root);
    env.jars
        .push(root.join("robots/testbots/build/libs/testbots-0.1.0-SNAPSHOT.jar"));
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 4,
            ..Default::default()
        },
        seed: 11,
        deterministic: true,
        hard_limit: std::time::Duration::from_secs(20),
        ..Default::default()
    };
    let robots = ["testbots.Pool".to_string(), "sample.Crazy".to_string()];
    let fight = || {
        let (host, battle) = prepare(&env, &robots, config.clone(), 256).unwrap();
        run_local_battle(&host, &battle, None, |_| {})
            .unwrap()
            .outcome
    };
    let first = fight();
    assert_eq!(first.hard_limits, [false, false]);
    assert_eq!(fight(), first);
}
