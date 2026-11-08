// SPDX-License-Identifier: EPL-2.0

//! A robot whose JVM runs out of memory is reported (#178): when that happens depends on the
//! machine, so the battle is treated like one with the hard limit (ADR 0025). The other
//! robots aren't.
//! Needs Java and the built robot host, agent and robots (`./gradlew
//! :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
//! :robots:testbots:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-runner --test out_of_memory -- --ignored` (`ROBOCODES_JAVA` names the JDK to
//! use).

use robocode_s_engine::BattleRules;
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::Path;

#[test]
#[ignore = "needs Java and the built robots"]
fn robots_that_run_out_of_memory_are_reported() {
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
    // testbots.Hog doesn't catch the error (classic's robot thread does), testbots.Glutton
    // catches it itself and fights on.
    let robots = [
        "sample.Fire".to_string(),
        "testbots.Hog".to_string(),
        "testbots.Glutton".to_string(),
        "sample.Walls".to_string(),
    ];
    let (host, battle) = prepare(&env, &robots, config, 128).unwrap();
    let local = run_local_battle(&host, &battle, None, |_| {}).unwrap();
    assert_eq!(local.outcome.rounds, 3);
    assert_eq!(local.outcome.out_of_memory, [false, true, true, false]);
    assert_eq!(local.outcome.hard_limits, [false; 4]);
}
