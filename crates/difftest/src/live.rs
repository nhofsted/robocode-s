// SPDX-License-Identifier: EPL-2.0

//! A live battle: the server, and every robot as its own JVM through the runner and the
//! Java robot host, compared with a classic recording of the same battle. In
//! deterministic mode the result must be identical to classic.

use crate::{RACE, classic_race, compare_snapshot, load, rounds, rules, setups, states};
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::JavaHost;
use robocode_s_runner::housekeeping::keep_battles;
use robocode_s_runner::local::{LocalBattle, LocalRobot, lookup_name, run_local_battle};
use robocode_s_runner::pool::Pool;
use robocode_s_server::ServerConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Runs the battle of `recording` live and compares it with the recording; with a `pool`,
/// the robots' hosts are warm ones (they must change nothing).
pub fn live_battle(
    recording: &Path,
    host: &JavaHost,
    jars: &[PathBuf],
    work: &Path,
    pool: Option<&Arc<Pool>>,
) -> Result<String, String> {
    let lines = load(recording);
    let header = &lines[0];
    let rounds = rounds(&lines);
    let infos = header["robotInfo"].as_array().cloned().unwrap_or_default();
    let setups = setups(header);
    let robots: Vec<LocalRobot> = setups
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let team_members: Vec<String> = match s.team {
                Some(t) => setups
                    .iter()
                    .filter(|o| o.team == Some(t))
                    .map(|o| o.name.clone())
                    .collect(),
                None => Vec::new(),
            };
            LocalRobot {
                lookup: lookup_name(&s.name).to_string(),
                statics: v1::RobotStatics {
                    name: s.name.clone(),
                    full_class_name: s.full_class_name.clone(),
                    r#type: Some(v1::RobotType {
                        advanced: s.advanced,
                        team: s.team_robot,
                        droid: s.droid,
                        sentry: s.sentry,
                        ..Default::default()
                    }),
                    team_name: s
                        .team
                        .map(|_| infos[i]["team"].as_str().unwrap_or("").to_string()),
                    team_index: s.team.map_or(-1, |t| t as i32),
                    team_leader: s.leader,
                    team_members,
                    ..Default::default()
                },
            }
        })
        .collect();
    let names: Vec<String> = setups.iter().map(|s| s.name.clone()).collect();
    let battle_name = recording
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let battle = LocalBattle {
        config: ServerConfig {
            rules: rules(header),
            seed: header["seed"].as_i64().unwrap_or(0),
            deterministic: true,
            hard_limit: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(120),
            ..Default::default()
        },
        robots,
        jars: jars.to_vec(),
        dir: work.join(&battle_name),
        // Battles that must repeat the oracle's start with no data files from earlier ones.
        data: None,
        memory_mb: 512,
        pool: pool.cloned(),
    };
    if let Some(pool) = pool {
        pool.warm(host, battle.memory_mb, true, battle.robots.len());
    }
    let mut snapshots = Vec::new();
    let local = run_local_battle(host, &battle, None, |msg| {
        if let server_message::Msg::TurnSnapshot(s) = msg {
            snapshots.push(s.clone());
        }
    })?;
    let mut stderr = String::new();
    for (i, tail) in local.stderr.iter().enumerate() {
        if !tail.is_empty() {
            stderr.push_str(&format!("\n--- robot {i} ({}) stderr:\n{tail}", names[i]));
        }
    }
    // Each robot's console output, for diagnosis: `<work>/<battle>/output-<i>.txt`, in a
    // battle's folder that stays (`ROBOCODES_KEEP_BATTLES`).
    for i in (0..names.len()).filter(|_| keep_battles()) {
        let mut text = String::new();
        for s in &snapshots {
            if let Some(r) = s.robots.get(i).filter(|r| !r.output.is_empty()) {
                for line in r.output.lines() {
                    text.push_str(&format!("{} {}: {line}\n", s.round, s.turn));
                }
            }
        }
        let _ = std::fs::write(battle.dir.join(format!("output-{i}.txt")), text);
    }
    let states = states(&rounds);
    for (snapshot, line) in snapshots.iter().zip(&states) {
        if let Err(e) = compare_snapshot(snapshot, line) {
            let round = line["round"].as_i64().unwrap_or(-1);
            let turn = line["turn"].as_i64().unwrap_or(0);
            return Err(match classic_race(&lines, round, turn) {
                Some(race) => format!("{RACE}: {race}; {e}"),
                None => format!("{e}{stderr}"),
            });
        }
    }
    if states.len() != snapshots.len() {
        return Err(format!(
            "{} states recorded, the live battle produced {} snapshots{stderr}",
            states.len(),
            snapshots.len()
        ));
    }
    let scores: Vec<String> = local
        .outcome
        .results
        .iter()
        .map(|r| format!("{} {:.0}", r.name, r.score.unwrap_or_default()))
        .collect();
    Ok(format!(
        "{} turns identical; scores: {}",
        snapshots.len(),
        scores.join(", ")
    ))
}
