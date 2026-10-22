// SPDX-License-Identifier: EPL-2.0

//! `live <recording>...`: replays the battle of each classic recording with real robot
//! processes (runner + Java robot host) and compares it with the recording.
//!
//! Paths come from the environment, with defaults for a built source tree:
//! `ROBOCODES_JAVA` (else the pinned JRE is downloaded into `ROBOCODES_DATA`),
//! `ROBOCODES_HOST_LIB`, `ROBOCODES_AGENT_JAR`, `ROBOCODES_ROBOT_JARS` (path list),
//! `ROBOCODES_SANDBOX` (else the platform's default). `ROBOCODES_WARM=1` starts the robots
//! from a warm pool (#82): the battle must come out the same.

use robocode_s_runner::pool::Pool;
use robocode_s_runner::{JavaHost, Sandbox, jre};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

fn main() -> ExitCode {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let data = env_path("ROBOCODES_DATA", "target/robocodes-data");
    let java = match jre::java(&data) {
        Ok(java) => java,
        Err(e) => {
            eprintln!("no Java runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The platform's default sandbox, or `ROBOCODES_SANDBOX`: a sandbox changes no battle.
    let sandbox = match std::env::var("ROBOCODES_SANDBOX") {
        Ok(name) => match name.parse::<Sandbox>() {
            Ok(sandbox) => sandbox,
            Err(e) => {
                eprintln!("ROBOCODES_SANDBOX: {e}");
                return ExitCode::FAILURE;
            }
        },
        Err(_) => Sandbox::default(),
    };
    if let Err(e) = sandbox.prepare() {
        eprintln!("ROBOCODES_SANDBOX: {e}");
        return ExitCode::FAILURE;
    }
    eprintln!("sandbox {}", sandbox.report());
    let host = JavaHost {
        java,
        lib_dir: env_path(
            "ROBOCODES_HOST_LIB",
            "clients/java-compat/build/install/robot-host/lib",
        ),
        agent_jar: env_path(
            "ROBOCODES_AGENT_JAR",
            "clients/java-determinism/build/libs/java-determinism-0.1.0-SNAPSHOT.jar",
        ),
        sandbox,
    };
    let jars: Vec<PathBuf> = std::env::var_os("ROBOCODES_ROBOT_JARS")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_else(|| {
            vec![PathBuf::from(
                "robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar",
            )]
        });
    let work = data.join("battles");
    let pool = std::env::var_os("ROBOCODES_WARM")
        .is_some_and(|v| v == "1")
        .then(|| {
            Arc::new(Pool::new(
                data.join(format!("pool-live-{}", std::process::id())),
            ))
        });
    let mut failed = 0;
    for file in &files {
        match robocode_s_difftest::live::live_battle(
            Path::new(file),
            &host,
            &jars,
            &work,
            pool.as_ref(),
        ) {
            Ok(summary) => {
                println!(
                    "OK   {file}: {summary}, engine {}",
                    robocode_s_engine::ENGINE_VERSION
                )
            }
            // A known classic race (engine-spec §15.3): reported, not a failure.
            Err(e) if e.starts_with(robocode_s_difftest::RACE) => println!("RACE {file}: {e}"),
            Err(e) => {
                failed += 1;
                println!("FAIL {file}: {e}");
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
