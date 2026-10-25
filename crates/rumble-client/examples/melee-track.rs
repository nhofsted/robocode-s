// SPDX-License-Identifier: EPL-2.0

//! Runs one melee round of the bundled sample robots and writes its viewer track, for the
//! website's replay fixtures: `cargo run -p robocode-s-rumble-client --example melee-track
//! -- OUT.track [SEED]`. Needs Java and the built robot host and samples, as the ignored
//! tests do (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`); `ROBOCODES_HOST_LIB`, `ROBOCODES_AGENT_JAR`, `SAMPLES_JAR` and
//! `ROBOCODES_JAVA` name them when they are elsewhere.

use robocode_s_runner::Sandbox;
use robocode_s_runner::local::Environment;
use rumble_client::battle::{Options, Runner};
use rumble_core::api::{BattleKind, BattleStatus, Rules, WorkBattle, WorkBot};
use rumble_core::ids::Division;
use std::path::{Path, PathBuf};

const ROBOTS: [&str; 8] = [
    "sample.Walls",
    "sample.Crazy",
    "sample.SpinBot",
    "sample.Tracker",
    "sample.TrackFire",
    "sample.RamFire",
    "sample.Corners",
    "sample.VelociRobot",
];

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

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: melee-track OUT.track [SEED]");
    let seed: i64 = args.next().map_or(7, |s| s.parse().expect("seed"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.sandbox = Sandbox::None;
    let samples: PathBuf = std::env::var_os("SAMPLES_JAR").map_or_else(
        || root.join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar"),
        PathBuf::from,
    );
    let mut options = Options::new(env);
    options.allow_unsandboxed = true;
    let runner = Runner::new(options).expect("runner");
    let battle = WorkBattle {
        index: 0,
        kind: BattleKind::Normal,
        division: Division::Melee,
        seed,
        rules: Rules {
            rounds: 1,
            width: 1000,
            height: 1000,
            gun_cooling_rate: 0.1,
            inactivity_time: 450,
            sentry_border: 100,
            hide_enemy_names: false,
        },
        bots: ROBOTS.iter().map(|b| bot(b)).collect(),
    };
    let outcome = runner.run(&battle, &vec![samples; ROBOTS.len()]);
    assert_eq!(
        outcome.result.status,
        BattleStatus::Ok,
        "{:?} {:?}",
        outcome.why,
        outcome.result.failure
    );
    std::fs::write(&out, &outcome.tracks[0]).expect("write");
    eprintln!(
        "{out}: {} bytes, robots {ROBOTS:?}",
        outcome.tracks[0].len()
    );
}
