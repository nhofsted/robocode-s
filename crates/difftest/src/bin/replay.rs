// SPDX-License-Identifier: EPL-2.0

//! `replay <recording>...`: replays classic recordings in the engine and reports the
//! first difference per file.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: replay <recording.jsonl[.gz]>...");
        return ExitCode::from(2);
    }
    let mut failed = 0;
    for file in &files {
        match robocode_s_difftest::replay_file(Path::new(file)) {
            Ok(s) => println!(
                "OK   {file}: {} rounds, {} turns, {} events",
                s.rounds, s.turns, s.events
            ),
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
