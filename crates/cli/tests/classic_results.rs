// SPDX-License-Identifier: EPL-2.0

//! `robocode-s battle` against classic: for each recording in `$ROBOCODES_RECORDINGS`
//! (`tools/difftest/record-samples.sh`), the same battle fought by the cli in
//! deterministic mode must name the robots as classic did and end with classic's
//! results. Skipped without recordings; needs the built robot host and sample robots
//! (see `tools/difftest/README.md`).

use flate2::read::GzDecoder;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

fn load(path: &Path) -> Vec<Value> {
    let file = std::fs::File::open(path).unwrap();
    BufReader::new(GzDecoder::new(file))
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect()
}

/// A name without its ` (n)` suffix.
fn strip_suffix(name: &str) -> &str {
    match name.rfind(" (") {
        Some(i) if name.ends_with(')') => &name[..i],
        _ => name,
    }
}

/// The robots as they were selected for classic: each team once, by its name.
fn selection(header: &Value) -> Vec<String> {
    let mut robots = Vec::new();
    let mut teams = Vec::new();
    for r in header["robotInfo"].as_array().unwrap() {
        if r["teamIndex"].as_i64().unwrap_or(-1) >= 0 {
            let team = r["team"].as_str().unwrap();
            if !teams.contains(&team) {
                teams.push(team);
                robots.push(strip_suffix(team).to_string());
            }
        } else {
            robots.push(strip_suffix(r["name"].as_str().unwrap()).to_string());
        }
    }
    robots
}

fn recordings() -> Vec<PathBuf> {
    let Some(dir) = std::env::var_os("ROBOCODES_RECORDINGS") else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".jsonl.gz"))
        .collect();
    files.sort();
    files
}

#[test]
fn battles_end_with_classic_results() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = recordings();
    if files.is_empty() {
        eprintln!("ROBOCODES_RECORDINGS not set: skipped");
        return;
    }
    let mut failures = Vec::new();
    let records = std::env::temp_dir().join(format!("robocodes-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&records).unwrap();
    for file in &files {
        let record = records.join("battle.rsr");
        let lines = load(file);
        let header = &lines[0];
        let classic = lines
            .iter()
            .rev()
            .find(|l| l["type"] == "results")
            .expect("results line");
        let robots = selection(header);
        let output = Command::new(env!("CARGO_BIN_EXE_robocode-s"))
            .current_dir(&root)
            .arg("battle")
            .arg(robots.join(","))
            .args(["--seed", &header["seed"].to_string()])
            .args(["--rounds", &header["rounds"].to_string()])
            .args(["--width", &header["width"].to_string()])
            .args(["--height", &header["height"].to_string()])
            .args(["--gun-cooling-rate", &header["gunCoolingRate"].to_string()])
            .args(["--inactivity-time", &header["inactivityTime"].to_string()])
            .args(["--sentry-border", &header["sentryBorderSize"].to_string()])
            .args(
                header["hideEnemyNames"]
                    .as_bool()
                    .unwrap_or(false)
                    .then_some("--hide-enemy-names"),
            )
            .arg("--json")
            .arg("--record")
            .arg(&record)
            .output()
            .unwrap();
        let name = file.file_name().unwrap().to_string_lossy();
        if !output.status.success() {
            failures.push(format!(
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }
        let ours: Value = serde_json::from_slice(&output.stdout).unwrap();
        if ours["robots"] != header["robots"] {
            failures.push(format!(
                "{name}: robots {} instead of {}",
                ours["robots"], header["robots"]
            ));
            continue;
        }
        // Classic lists contestants by index (its `rank` is the position); compare by name.
        for expected in classic["results"].as_array().unwrap() {
            let contestant = expected["name"].as_str().unwrap();
            let Some(actual) = ours["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["name"] == contestant)
            else {
                failures.push(format!("{name}: no results for {contestant}"));
                continue;
            };
            for field in [
                "score",
                "survival",
                "lastSurvivorBonus",
                "bulletDamage",
                "bulletDamageBonus",
                "ramDamage",
                "ramDamageBonus",
                "firsts",
                "seconds",
                "thirds",
            ] {
                // Classic reports `(int) (value + 0.5)`.
                let ours = (actual[field].as_f64().unwrap() + 0.5) as i64;
                let theirs = expected[field].as_i64().unwrap();
                if ours != theirs {
                    failures.push(format!(
                        "{name}: {contestant} {field} {ours}, classic {theirs}"
                    ));
                }
            }
        }
        // The recording replays to the same battle and results.
        let replay = Command::new(env!("CARGO_BIN_EXE_robocode-s"))
            .args(["replay", "--json"])
            .arg(&record)
            .output()
            .unwrap();
        let replayed: Value = serde_json::from_slice(&replay.stdout).unwrap_or_default();
        if !replay.status.success() || replayed["results"] != ours["results"] {
            failures.push(format!(
                "{name}: replay of the recording differs: {}",
                String::from_utf8_lossy(&replay.stderr)
            ));
        }
        eprintln!("{name}: checked");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
