// SPDX-License-Identifier: EPL-2.0

//! `robocode-s` command-line interface.

mod agent;
mod battle;
mod replay;
mod results;

use std::process::ExitCode;

const USAGE: &str = "\
usage: robocode-s <command> [options]

Commands:
  battle     run a headless battle and print the results
  replay     re-run a recorded battle, verify it and print the results
  agent      run robots for battles on other machines (a runner agent)
  --version  print the version, engine version and protocol version

`robocode-s battle --help` describes the battle options, `robocode-s agent --help`
the agent's.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            let protocol = robocode_s_protocol::PROTOCOL_VERSION;
            println!(
                "robocode-s {} (engine {}, protocol {}.{})",
                env!("CARGO_PKG_VERSION"),
                robocode_s_engine::ENGINE_VERSION,
                protocol.major,
                protocol.minor
            );
            ExitCode::SUCCESS
        }
        Some("battle") => {
            if args[1..].iter().any(|a| a == "--help" || a == "-h") {
                print!("{}", battle::USAGE);
                return ExitCode::SUCCESS;
            }
            let result = battle::Options::parse(&args[1..])
                .map_err(|e| format!("{e}\n\n{}", battle::USAGE))
                .and_then(|o| battle::run(&o));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("robocode-s battle: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("agent") => {
            if args[1..].iter().any(|a| a == "--help" || a == "-h") {
                print!("{}", agent::USAGE);
                return ExitCode::SUCCESS;
            }
            let result = agent::Options::parse(&args[1..])
                .map_err(|e| format!("{e}\n\n{}", agent::USAGE))
                .and_then(|o| agent::run_agent(&o));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("robocode-s agent: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("replay") => {
            if args[1..].iter().any(|a| a == "--help" || a == "-h") {
                print!("{}", replay::USAGE);
                return ExitCode::SUCCESS;
            }
            match replay::run(&args[1..]) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("robocode-s replay: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
