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
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
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
    ];
    let (host, battle) = prepare(&env, &robots, config, 256).unwrap();
    let local = run_local_battle(&host, &battle, None, |_| {}).unwrap();
    // By robot index: a Timer's thread, and only that; the scheduler runs testbots.Threads'
    // own thread deterministically.
    assert_eq!(local.outcome.threads, [false, false, false, true]);
}
