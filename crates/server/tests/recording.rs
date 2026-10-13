// SPDX-License-Identifier: EPL-2.0

//! A recorded battle replays to exactly what observers saw (design §11).

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::recording::{Frame, Recording};
use robocode_s_server::{RobotEntry, ServerConfig, run_battle};
use std::net::{TcpListener, TcpStream};
use std::thread;

fn entry(name: &str, token: &str) -> RobotEntry {
    RobotEntry {
        token: token.into(),
        statics: v1::RobotStatics {
            name: name.into(),
            full_class_name: name.into(),
            r#type: Some(v1::RobotType {
                advanced: true,
                ..Default::default()
            }),
            team_index: -1,
            ..Default::default()
        },
    }
}

fn connect(address: &str, role: v1::Role, token: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).unwrap();
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: role as i32,
            token: token.into(),
            ..Default::default()
        })),
    };
    write_message(&mut stream, &hello).unwrap();
    stream
}

/// A robot that drives, turns its gun and fires on a fixed pseudo-random script, and
/// prints something now and then.
fn scripted_robot(address: String, token: &'static str, seed: u64) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut stream = connect(&address, v1::Role::Robot, token);
        let mut state = seed;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as f64 / (1u64 << 31) as f64
        };
        let mut bullet_id = 0;
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut stream) {
            let turn = match message.msg {
                Some(server_message::Msg::RoundStart(_)) => 0,
                Some(server_message::Msg::Tick(t)) => t.turn,
                Some(server_message::Msg::BattleOver(_)) => break,
                _ => continue,
            };
            let mut commands = v1::RobotCommands {
                body_turn_remaining: Some((next() - 0.5) * 2.0),
                gun_turn_remaining: Some((next() - 0.5) * 3.0),
                radar_turn_remaining: Some(next() * 6.0),
                distance_remaining: Some((next() - 0.3) * 200.0),
                max_turn_rate: Some(10.0),
                max_velocity: Some(8.0),
                ..Default::default()
            };
            if next() < 0.2 {
                bullet_id += 1;
                commands.bullets.push(v1::BulletCommand {
                    bullet_id,
                    power: Some(0.1 + next() * 2.9),
                    ..Default::default()
                });
            }
            let output = if turn % 37 == 5 {
                format!("{token} at turn {turn}\n")
            } else {
                String::new()
            };
            let execute = v1::ClientMessage {
                msg: Some(client_message::Msg::Execute(v1::Execute {
                    turn,
                    commands: Some(commands),
                    output,
                    ..Default::default()
                })),
            };
            if write_message(&mut stream, &execute).is_err() {
                break;
            }
        }
    })
}

#[test]
fn a_recorded_battle_replays_exactly() {
    let dir = std::env::temp_dir().join(format!("robocodes-recording-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("battle.rsr");
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            width: 400,
            height: 400,
            ..Default::default()
        },
        seed: 7,
        deterministic: true,
        record: Some(path.clone()),
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let robots = vec![entry("a.A", "a"), entry("b.B", "b")];
    let server = thread::spawn(move || run_battle(listener, &config, robots));

    let mut observer = connect(&address, v1::Role::Observer, "");
    let watcher = thread::spawn(move || {
        let mut frames = Vec::new();
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut observer) {
            match message.msg {
                Some(server_message::Msg::TurnSnapshot(snapshot)) => frames.push(Frame::Snapshot {
                    snapshot,
                    verified: true,
                }),
                Some(server_message::Msg::RoundResult(r)) => frames.push(Frame::RoundOver(r)),
                _ => {}
            }
        }
        frames
    });
    let a = scripted_robot(address.clone(), "a", 1);
    let b = scripted_robot(address, "b", 2);
    let outcome = server.join().unwrap().unwrap();
    let seen = watcher.join().unwrap();
    a.join().unwrap();
    b.join().unwrap();

    let recording = Recording::read(&path).unwrap();
    assert_eq!(recording.header.seed, 7);
    assert_eq!(recording.header.robots.len(), 2);
    let mut replay = recording.replay();
    let replayed: Vec<Frame> = replay.by_ref().collect();
    assert!(replay.is_done());
    assert!(seen.len() > 100, "{} frames seen", seen.len());
    assert!(
        seen.iter()
            .any(|f| matches!(f, Frame::Snapshot { snapshot, .. }
            if snapshot.robots.iter().any(|r| !r.output.is_empty()))),
        "no console output in the battle"
    );
    assert_eq!(replayed.len(), seen.len());
    for (i, (r, s)) in replayed.iter().zip(&seen).enumerate() {
        assert_eq!(r, s, "frame {i} differs");
    }
    let final_results = robocode_s_server::recording::round_result(replay.battle()).results;
    assert_eq!(final_results, outcome.results);
    let _ = std::fs::remove_dir_all(&dir);
}
