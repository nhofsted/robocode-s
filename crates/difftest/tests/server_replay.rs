// SPDX-License-Identifier: EPL-2.0

//! Replays classic recordings through the battle server over TCP: every robot is a
//! client that sends exactly the commands classic loaded, and checks the events and
//! remaining values in its `Tick`s; an observer checks every `TurnSnapshot`.
//! Uses the recordings in `$ROBOCODES_RECORDINGS`; skipped when it is not set.

use flate2::read::GzDecoder;
use robocode_s_difftest::{
    describe, events_equal, json_number, number, parse_commands, rules, same, setups,
};
use robocode_s_engine::Commands;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::{RobotEntry, ServerConfig, run_battle};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn load(path: &Path) -> Vec<Value> {
    let file = std::fs::File::open(path).unwrap();
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    BufReader::new(reader)
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect()
}

/// One round of the recording: the start line and the turn lines (index = turn - 1).
struct Round {
    start: Value,
    turns: Vec<Value>,
}

fn rounds(lines: &[Value]) -> Vec<Round> {
    let mut rounds: Vec<Round> = Vec::new();
    for line in lines {
        match line["type"].as_str() {
            Some("roundStart") => rounds.push(Round {
                start: line.clone(),
                turns: Vec::new(),
            }),
            Some("turn") => rounds.last_mut().unwrap().turns.push(line.clone()),
            _ => {}
        }
    }
    rounds
}

fn to_proto(c: &Commands) -> v1::RobotCommands {
    v1::RobotCommands {
        body_turn_remaining: Some(c.body_turn_remaining),
        gun_turn_remaining: Some(c.gun_turn_remaining),
        radar_turn_remaining: Some(c.radar_turn_remaining),
        distance_remaining: Some(c.distance_remaining),
        adjust_gun_for_body_turn: c.adjust_gun_for_body_turn,
        adjust_radar_for_gun_turn: c.adjust_radar_for_gun_turn,
        adjust_radar_for_body_turn: c.adjust_radar_for_body_turn,
        max_turn_rate: Some(c.max_turn_rate),
        max_velocity: Some(c.max_velocity),
        scan: c.scan,
        io_robot: c.io,
        bullets: c
            .bullets
            .iter()
            .filter(|b| !b.consumed)
            .map(|b| v1::BulletCommand {
                bullet_id: b.id,
                power: Some(b.power),
                fire_assist_valid: b.assist_valid,
                fire_assist_angle: Some(b.assist_angle),
            })
            .collect(),
        team_messages: c
            .messages
            .iter()
            .map(|m| v1::TeamMessage {
                sender: m.sender.clone(),
                recipient: m.recipient.clone(),
                payload: m.bytes.clone(),
            })
            .collect(),
        ..Default::default()
    }
}

fn bullet_json(b: &Option<v1::Bullet>) -> Value {
    let b = b.clone().unwrap_or_default();
    json!({"id": b.bullet_id, "owner": b.owner, "victim": b.victim.unwrap_or_else(|| "null".into()),
           "heading": json_number(b.heading.unwrap_or_default()), "x": json_number(b.x.unwrap_or_default()), "y": json_number(b.y.unwrap_or_default()),
           "power": json_number(b.power.unwrap_or_default()), "active": b.active})
}

/// The oracle's JSON form of a protocol event.
fn proto_event_json(e: &v1::Event) -> Value {
    use v1::event::Kind;
    match e.kind.as_ref().unwrap() {
        Kind::ScannedRobot(s) => {
            json!({"type": "ScannedRobotEvent", "name": s.name, "energy": json_number(s.energy.unwrap_or_default()),
            "bearing": json_number(s.bearing.unwrap_or_default()), "distance": json_number(s.distance.unwrap_or_default()),
            "heading": json_number(s.heading.unwrap_or_default()), "velocity": json_number(s.velocity.unwrap_or_default()), "sentry": s.sentry})
        }
        Kind::HitWall(h) => {
            json!({"type": "HitWallEvent", "bearing": json_number(h.bearing.unwrap_or_default())})
        }
        Kind::HitRobot(h) => {
            json!({"type": "HitRobotEvent", "name": h.name, "bearing": json_number(h.bearing.unwrap_or_default()),
            "energy": json_number(h.energy.unwrap_or_default()), "atFault": h.at_fault})
        }
        Kind::HitByBullet(h) => {
            json!({"type": "HitByBulletEvent", "bearing": json_number(h.bearing.unwrap_or_default()),
            "bullet": bullet_json(&h.bullet)})
        }
        Kind::BulletHit(h) => {
            json!({"type": "BulletHitEvent", "name": h.name, "energy": json_number(h.energy.unwrap_or_default()),
            "bullet": bullet_json(&h.bullet)})
        }
        Kind::BulletHitBullet(h) => {
            json!({"type": "BulletHitBulletEvent", "bullet": bullet_json(&h.bullet),
            "hitBullet": bullet_json(&h.hit_bullet)})
        }
        Kind::BulletMissed(h) => {
            json!({"type": "BulletMissedEvent", "bullet": bullet_json(&h.bullet)})
        }
        Kind::RobotDeath(d) => json!({"type": "RobotDeathEvent", "name": d.name}),
        Kind::Death(_) => json!({"type": "DeathEvent"}),
        Kind::Win(_) => json!({"type": "WinEvent"}),
        Kind::RoundEnded(r) => {
            json!({"type": "RoundEndedEvent", "round": r.round, "turns": r.turns,
            "totalTurns": r.total_turns})
        }
        Kind::BattleEnded(b) => {
            let r = b.results.clone().unwrap_or_default();
            json!({"type": "BattleEndedEvent", "aborted": b.aborted, "rank": r.rank,
                   "score": (r.score.unwrap_or_default() + 0.5) as i64, "survival": (r.survival.unwrap_or_default() + 0.5) as i64, "firsts": r.firsts})
        }
        Kind::SkippedTurn(s) => json!({"type": "SkippedTurnEvent", "skippedTurn": s.turn}),
    }
}

fn send(stream: &mut TcpStream, msg: client_message::Msg) {
    write_message(stream, &v1::ClientMessage { msg: Some(msg) }).unwrap();
}

fn receive(stream: &mut TcpStream) -> Option<server_message::Msg> {
    read_message::<v1::ServerMessage>(stream)
        .unwrap()
        .and_then(|m| m.msg)
}

fn hello(stream: &mut TcpStream, role: v1::Role, token: &str) {
    send(
        stream,
        client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: role as i32,
            token: token.into(),
            ..Default::default()
        }),
    );
}

/// A robot that replays classic's commands. Returns the number of ticks it checked.
fn robot(
    index: usize,
    address: String,
    rounds: Arc<Vec<Round>>,
    errors: Arc<Mutex<Vec<String>>>,
) -> usize {
    let mut stream = TcpStream::connect(address).unwrap();
    hello(&mut stream, v1::Role::Robot, &format!("token-{index}"));
    let commands = |round: usize, turn: usize| -> v1::RobotCommands {
        let line = rounds[round].turns.get(turn - 1);
        line.map(|l| &l["commands"][index])
            .filter(|c| !c.is_null())
            .map(|c| to_proto(&parse_commands(c)))
            .unwrap_or_default()
    };
    let fail = |message: String| {
        let mut e = errors.lock().unwrap();
        if e.len() < 5 {
            e.push(message);
        }
    };
    let mut round = 0usize;
    let mut ticks = 0;
    while let Some(msg) = receive(&mut stream) {
        match msg {
            server_message::Msg::RoundStart(r) => {
                round = r.round as usize;
                let execute = v1::Execute {
                    turn: 0,
                    commands: Some(commands(round, 1)),
                    ..Default::default()
                };
                send(&mut stream, client_message::Msg::Execute(execute));
            }
            server_message::Msg::Tick(tick) => {
                ticks += 1;
                let turn = tick.turn as usize;
                let line = &rounds[round].turns[turn - 1];
                let expected = line["events"][index]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let actual: Vec<Value> = tick.events.iter().map(proto_event_json).collect();
                if !events_equal(&expected, &actual) {
                    fail(format!(
                        "robot {index} round {round} turn {turn} events:\n  classic {}\n  server  {}",
                        Value::Array(expected),
                        Value::Array(actual)
                    ));
                }
                let remaining = &line["remaining"][index];
                if !remaining.is_null() {
                    let status = tick.status.unwrap_or_default();
                    for (name, actual) in [
                        ("bodyTurn", status.body_turn_remaining.unwrap_or_default()),
                        ("gunTurn", status.gun_turn_remaining.unwrap_or_default()),
                        ("radarTurn", status.radar_turn_remaining.unwrap_or_default()),
                        ("distance", status.distance_remaining.unwrap_or_default()),
                    ] {
                        let expected = number(&remaining[name]);
                        if !same(expected, actual) {
                            fail(format!(
                                "robot {index} round {round} turn {turn} remaining {name}: classic {}, server {}",
                                describe(expected),
                                describe(actual)
                            ));
                        }
                    }
                }
                // A halted robot still sends its final commands (classic stores them before
                // stopping); a dead one has none.
                let next = rounds[round].turns.get(turn).map(|l| &l["commands"][index]);
                if !tick.halt || next.is_some_and(|c| !c.is_null()) {
                    let execute = v1::Execute {
                        turn: tick.turn,
                        commands: Some(commands(round, turn + 1)),
                        ..Default::default()
                    };
                    send(&mut stream, client_message::Msg::Execute(execute));
                }
            }
            server_message::Msg::BattleOver(_) => break,
            _ => {}
        }
    }
    ticks
}

/// Compares an observer snapshot with a recorded state line.
fn compare_snapshot(snapshot: &v1::TurnSnapshot, line: &Value) -> Result<(), String> {
    let at = format!("round {} turn {}", snapshot.round, snapshot.turn);
    if line["turn"].as_i64() != Some(snapshot.turn)
        || line["round"].as_i64() != Some(i64::from(snapshot.round))
    {
        return Err(format!(
            "{at}: snapshot out of step with the recording ({}/{})",
            line["round"], line["turn"]
        ));
    }
    let states = ["ACTIVE", "HIT_WALL", "HIT_ROBOT", "DEAD"];
    for (i, (r, e)) in line["robots"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&snapshot.robots)
        .enumerate()
    {
        if r["state"].as_str() != Some(states[e.state as usize]) {
            return Err(format!(
                "{at}: robot {i} state: classic {}, server {}",
                r["state"], states[e.state as usize]
            ));
        }
        for (name, actual) in [
            ("x", e.x.unwrap_or_default()),
            ("y", e.y.unwrap_or_default()),
            ("energy", e.energy.unwrap_or_default()),
            ("velocity", e.velocity.unwrap_or_default()),
            ("bodyHeading", e.body_heading.unwrap_or_default()),
            ("gunHeading", e.gun_heading.unwrap_or_default()),
            ("radarHeading", e.radar_heading.unwrap_or_default()),
            ("gunHeat", e.gun_heat.unwrap_or_default()),
            ("score", e.score.unwrap_or_default()),
            ("totalScore", e.total_score.unwrap_or_default()),
        ] {
            if !same(number(&r[name]), actual) {
                return Err(format!(
                    "{at}: robot {i} {name}: classic {}, server {}",
                    describe(number(&r[name])),
                    describe(actual)
                ));
            }
        }
    }
    let bullets = line["bullets"].as_array().unwrap();
    if bullets.len() != snapshot.bullets.len() {
        return Err(format!(
            "{at}: {} bullets recorded, server sent {}",
            bullets.len(),
            snapshot.bullets.len()
        ));
    }
    for (k, (b, e)) in bullets.iter().zip(&snapshot.bullets).enumerate() {
        let ok = b["owner"].as_i64() == Some(i64::from(e.owner))
            && b["id"].as_i64() == Some(i64::from(e.bullet_id))
            && b["victim"].as_i64() == Some(i64::from(e.victim))
            && same(number(&b["x"]), e.x.unwrap_or_default())
            && same(number(&b["y"]), e.y.unwrap_or_default())
            && same(number(&b["heading"]), e.heading.unwrap_or_default())
            && same(number(&b["power"]), e.power.unwrap_or_default());
        if !ok {
            return Err(format!(
                "{at}: bullet {k} differs: classic {b}, server {e:?}"
            ));
        }
    }
    Ok(())
}

fn replay_through_server(path: &Path) -> Result<String, String> {
    let lines = load(path);
    let header = &lines[0];
    let rounds = Arc::new(rounds(&lines));
    let robots: Vec<RobotEntry> = setups(header)
        .iter()
        .enumerate()
        .map(|(i, s)| RobotEntry {
            token: format!("token-{i}"),
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
                team_index: s.team.map_or(-1, |t| t as i32),
                team_leader: s.leader,
                ..Default::default()
            },
        })
        .collect();
    let count = robots.len();
    let config = ServerConfig {
        rules: rules(header),
        seed: header["seed"].as_i64().unwrap(),
        deterministic: true,
        hard_limit: Duration::from_secs(60),
        connect_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = thread::spawn(move || run_battle(listener, &config, robots));

    // The observer connects first, so it sees the battle from the start.
    let mut observer = TcpStream::connect(&address).unwrap();
    hello(&mut observer, v1::Role::Observer, "");
    assert!(matches!(
        receive(&mut observer),
        Some(server_message::Msg::HelloAck(_))
    ));
    assert!(matches!(
        receive(&mut observer),
        Some(server_message::Msg::BattleInfo(_))
    ));
    let observer = thread::spawn(move || {
        let mut snapshots = Vec::new();
        while let Some(msg) = receive(&mut observer) {
            if let server_message::Msg::TurnSnapshot(s) = msg {
                snapshots.push(s);
            }
        }
        snapshots
    });

    let errors = Arc::new(Mutex::new(Vec::new()));
    let clients: Vec<_> = (0..count)
        .map(|i| {
            let (address, rounds, errors) = (address.clone(), rounds.clone(), errors.clone());
            thread::spawn(move || robot(i, address, rounds, errors))
        })
        .collect();
    let ticks: usize = clients.into_iter().map(|c| c.join().unwrap()).sum();
    server.join().unwrap().map_err(|e| e.to_string())?;
    let snapshots = observer.join().unwrap();

    let errors = errors.lock().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let states: Vec<&Value> = rounds
        .iter()
        .flat_map(|r| std::iter::once(&r.start).chain(r.turns.iter()))
        .collect();
    if states.len() != snapshots.len() {
        return Err(format!(
            "{} states recorded, observer got {} snapshots",
            states.len(),
            snapshots.len()
        ));
    }
    for (snapshot, line) in snapshots.iter().zip(states) {
        compare_snapshot(snapshot, line)?;
    }
    Ok(format!("{} snapshots, {ticks} ticks", snapshots.len()))
}

#[test]
fn server_matches_classic_recordings() {
    let Ok(dir) = std::env::var("ROBOCODES_RECORDINGS") else {
        eprintln!("ROBOCODES_RECORDINGS not set; skipping");
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".jsonl"))
        .collect();
    files.sort();
    let mut failures = Vec::new();
    for file in &files {
        match replay_through_server(file) {
            Ok(summary) => eprintln!("OK   {}: {summary}", file.display()),
            Err(e) => failures.push(format!("FAIL {}: {e}", file.display())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
