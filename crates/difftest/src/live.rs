// SPDX-License-Identifier: EPL-2.0

//! A live battle: the server, and every robot as its own JVM through the runner and the
//! Java robot host, compared with a classic recording of the same battle. In
//! deterministic mode the result must be identical to classic.

use crate::{RACE, classic_race, compare_snapshot, load, rounds, rules, setups, states};
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_runner::{JavaHost, RobotLaunch, launch};
use robocode_s_server::{RobotEntry, ServerConfig, run_battle};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// The classic name to look a robot up by: its unique name without a duplicate suffix.
fn lookup_name(name: &str) -> &str {
    match name.rfind(" (") {
        Some(i) if name.ends_with(')') => &name[..i],
        _ => name,
    }
}

/// Runs the battle of `recording` live and compares it with the recording.
pub fn live_battle(
    recording: &Path,
    host: &JavaHost,
    jars: &[PathBuf],
    work: &Path,
) -> Result<String, String> {
    let lines = load(recording);
    let header = &lines[0];
    let rounds = rounds(&lines);
    let infos = header["robotInfo"].as_array().cloned().unwrap_or_default();
    let setups = setups(header);
    let robots: Vec<RobotEntry> = setups
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
            RobotEntry {
                token: format!("live-{i}"),
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
    let seed = header["seed"].as_i64().unwrap_or(0);
    let config = ServerConfig {
        rules: rules(header),
        seed,
        deterministic: true,
        hard_limit: Duration::from_secs(120),
        connect_timeout: Duration::from_secs(120),
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let address = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let names: Vec<String> = setups.iter().map(|s| s.name.clone()).collect();
    let server = thread::spawn(move || run_battle(listener, &config, robots));

    let mut observer = TcpStream::connect(&address).map_err(|e| e.to_string())?;
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Observer as i32,
            ..Default::default()
        })),
    };
    write_message(&mut observer, &hello).map_err(|e| e.to_string())?;
    let observer = thread::spawn(move || {
        let mut snapshots = Vec::new();
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut observer) {
            if let Some(server_message::Msg::TurnSnapshot(s)) = message.msg {
                snapshots.push(s);
            }
        }
        snapshots
    });

    let battle = recording
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut processes = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let robot = RobotLaunch {
            server: address.clone(),
            token: format!("live-{i}"),
            robot: lookup_name(name).to_string(),
            jars: jars.to_vec(),
            home: work.join(&battle).join(format!("robot-{i}")),
            seed,
            deterministic: true,
            memory_mb: 512,
        };
        processes.push(launch(host, &robot).map_err(|e| format!("launching {name}: {e}"))?);
    }
    let outcome = server
        .join()
        .map_err(|_| "server panicked".to_string())?
        .map_err(|e| e.to_string())?;
    let snapshots = observer
        .join()
        .map_err(|_| "observer panicked".to_string())?;
    let mut stderr = String::new();
    for (i, p) in processes.iter_mut().enumerate() {
        for _ in 0..100 {
            if p.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let tail = p.stderr_tail();
        let interesting: Vec<&str> = tail
            .lines()
            .filter(|l| {
                !l.starts_with("Loaded ")
                    && !l.contains("robocode.properties")
                    && !l.trim().is_empty()
            })
            .collect();
        if !interesting.is_empty() {
            stderr.push_str(&format!(
                "\n--- robot {i} ({}) stderr:\n{}",
                names[i],
                interesting.join("\n")
            ));
        }
    }
    // Each robot's console output, for diagnosis: `<work>/<battle>/output-<i>.txt`.
    for i in 0..names.len() {
        let mut text = String::new();
        for s in &snapshots {
            if let Some(r) = s.robots.get(i).filter(|r| !r.output.is_empty()) {
                for line in r.output.lines() {
                    text.push_str(&format!("{} {}: {line}\n", s.round, s.turn));
                }
            }
        }
        let _ = std::fs::write(work.join(&battle).join(format!("output-{i}.txt")), text);
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
    let scores: Vec<String> = outcome
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
