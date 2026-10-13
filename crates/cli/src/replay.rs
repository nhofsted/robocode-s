// SPDX-License-Identifier: EPL-2.0

//! `robocode-s replay`: plays a recording back through the engine, verifies every turn
//! against the recorded hashes and prints the results.

use crate::results;
use robocode_s_server::recording::{Frame, Recording, round_result};
use std::path::Path;

pub const USAGE: &str = "\
usage: robocode-s replay [--json] <recording>

Re-runs the recorded battle in the engine, checks every turn against the recording
and prints the results.
";

pub fn run(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [file] = files.as_slice() else {
        return Err(format!("one recording expected\n\n{USAGE}"));
    };
    let recording =
        Recording::read(Path::new(file.as_str())).map_err(|e| format!("{file}: {e}"))?;
    let mut replay = recording.replay();
    let (mut turns, mut painted, mut unverified) = (0usize, 0usize, Vec::new());
    for frame in replay.by_ref() {
        if let Frame::Snapshot { snapshot, verified } = frame {
            turns += 1;
            painted += usize::from(snapshot.robots.iter().any(|r| !r.paint.is_empty()));
            if !verified && unverified.len() < 5 {
                unverified.push(format!("round {} turn {}", snapshot.round, snapshot.turn));
            }
        }
    }
    let statics = &recording.header.robots;
    let rows = results::contestants(statics, &round_result(replay.battle()).results);
    if json {
        let out = serde_json::json!({
            "seed": recording.header.seed,
            "rounds": recording.rounds(),
            "turns": turns,
            "paintedTurns": painted,
            "verified": unverified.is_empty(),
            "robots": statics.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            "results": results::json(&rows),
        });
        println!("{out}");
    } else {
        println!(
            "{} rounds, seed {}, recorded by robocode-s {}; {turns} turns {}{}\n",
            recording.rounds(),
            recording.header.seed,
            recording.header.engine,
            if unverified.is_empty() {
                "replayed identically"
            } else {
                "replayed, NOT identical"
            },
            if painted > 0 {
                format!(", {painted} with debug paint")
            } else {
                String::new()
            }
        );
        print!("{}", results::table(&rows));
    }
    if unverified.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the replay differs from the recording at {}",
            unverified.join(", ")
        ))
    }
}
