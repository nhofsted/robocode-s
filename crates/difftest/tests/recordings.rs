// SPDX-License-Identifier: EPL-2.0

//! Replays every recording in `$ROBOCODES_RECORDINGS` (made by the classic oracle,
//! `./gradlew :tools:difftest:run`). Skipped when the variable is not set.

#[test]
fn engine_matches_classic_recordings() {
    let Ok(dir) = std::env::var("ROBOCODES_RECORDINGS") else {
        eprintln!("ROBOCODES_RECORDINGS not set; skipping");
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("recordings directory")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.to_string_lossy().contains(".jsonl"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no recordings in {dir}");
    let failures: Vec<String> = files
        .iter()
        .filter_map(|f| robocode_s_difftest::replay_file(f).err())
        // A known classic race (engine-spec §15.3) is not a robocode-s error.
        .filter(|e| !e.starts_with(robocode_s_difftest::RACE))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} recordings differ:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
