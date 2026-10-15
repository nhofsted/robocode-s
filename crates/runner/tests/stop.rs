// SPDX-License-Identifier: EPL-2.0

//! A local battle with real robot JVMs, stopped by its observer (as the GUI's stop button
//! does). Needs Java and the built robot host and sample robots
//! (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-runner -- --ignored` (`ROBOCODES_JAVA` names the JDK to use).

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::Path;
use std::sync::mpsc;

#[test]
#[ignore = "needs Java and the built robots"]
fn a_local_battle_stops_with_the_results_so_far() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 10,
            ..Default::default()
        },
        seed: 3,
        deterministic: true,
        ..Default::default()
    };
    let robots = ["sample.Fire".to_string(), "sample.Walls".to_string()];
    let (host, battle) = prepare(&env, &robots, config, 256).unwrap();
    let (controls, rx) = mpsc::channel();
    let mut last = (0, 0);
    let local = run_local_battle(&host, &battle, Some(rx), |msg| {
        if let server_message::Msg::TurnSnapshot(s) = msg {
            if (s.round, s.turn) == (0, 300) {
                let _ = controls.send(v1::ObserverControl {
                    control: Some(v1::observer_control::Control::Abort(true)),
                });
            }
            last = (s.round, s.turn);
        }
    })
    .unwrap();
    let outcome = local.outcome;
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 1);
    assert_eq!(last.0, 0, "no round after the stop");
    let mut ranks: Vec<i32> = outcome.results.iter().map(|r| r.rank).collect();
    ranks.sort();
    assert_eq!(ranks, [1, 2]);
    assert!(
        outcome
            .results
            .iter()
            .any(|r| r.score.unwrap_or_default() > 0.0),
        "{:?}",
        outcome.results
    );
}
