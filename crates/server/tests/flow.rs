// SPDX-License-Identifier: EPL-2.0

//! The server's connection handling and turn flow with small scripted robots.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, event, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::{BattleOutcome, RobotEntry, ServerConfig, run_battle};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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

fn start(
    config: ServerConfig,
    robots: Vec<RobotEntry>,
) -> (String, JoinHandle<std::io::Result<BattleOutcome>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    (
        address,
        thread::spawn(move || run_battle(listener, &config, robots)),
    )
}

fn connect(address: &str, version: u32, role: v1::Role, token: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).unwrap();
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(v1::ProtocolVersion {
                major: version,
                minor: 0,
            }),
            role: role as i32,
            token: token.into(),
            ..Default::default()
        })),
    };
    write_message(&mut stream, &hello).unwrap();
    stream
}

fn receive(stream: &mut TcpStream) -> Option<server_message::Msg> {
    read_message::<v1::ServerMessage>(stream)
        .ok()
        .flatten()
        .and_then(|m| m.msg)
}

fn execute(stream: &mut TcpStream, turn: i64) {
    let message = v1::ClientMessage {
        msg: Some(client_message::Msg::Execute(v1::Execute {
            turn,
            commands: Some(v1::RobotCommands::default()),
            ..Default::default()
        })),
    };
    let _ = write_message(stream, &message);
}

fn small_rules() -> BattleRules {
    BattleRules {
        num_rounds: 1,
        inactivity_time: 5,
        ..Default::default()
    }
}

#[test]
fn refuses_unknown_tokens_and_versions() {
    let config = ServerConfig {
        connect_timeout: Duration::from_millis(500),
        start_timeout: Duration::from_millis(50),
        turn_timeout: Duration::from_millis(1),
        rules: small_rules(),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "good")]);
    let mut wrong_token = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "bad");
    assert!(
        matches!(receive(&mut wrong_token), Some(server_message::Msg::Error(e)) if e.message.contains("token"))
    );
    let mut wrong_version = connect(&address, 99, v1::Role::Robot, "good");
    assert!(
        matches!(receive(&mut wrong_version), Some(server_message::Msg::Error(e)) if e.message.contains("version"))
    );
    // The robot never connected: it is disabled and the battle still finishes.
    let outcome = server.join().unwrap().unwrap();
    assert_eq!(outcome.results.len(), 1);
}

#[test]
fn classic_mode_skips_turns_of_slow_robots() {
    let config = ServerConfig {
        rules: small_rules(),
        turn_timeout: Duration::from_millis(20),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "a"), entry("b.B", "b")]);
    // A answers at once and later just disconnects; the battle continues with its commands.
    let fast = {
        let address = address.clone();
        thread::spawn(move || {
            let mut s = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "a");
            while let Some(msg) = receive(&mut s) {
                match msg {
                    server_message::Msg::RoundStart(_) => execute(&mut s, 0),
                    server_message::Msg::Tick(t) if t.turn >= 10 => return,
                    server_message::Msg::Tick(t) => execute(&mut s, t.turn),
                    _ => {}
                }
            }
        })
    };
    let mut slow = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "b");
    let mut skipped = Vec::new();
    while let Some(msg) = receive(&mut slow) {
        match msg {
            server_message::Msg::RoundStart(_) => execute(&mut slow, 0),
            server_message::Msg::Tick(t) if t.turn == 3 => {
                // Too slow: the next turns run without B.
                thread::sleep(Duration::from_millis(150));
                execute(&mut slow, t.turn);
            }
            server_message::Msg::Tick(t) if t.turn > 3 => {
                for e in &t.events {
                    if let Some(event::Kind::SkippedTurn(s)) = &e.kind {
                        skipped.push(s.turn);
                    }
                }
                break;
            }
            server_message::Msg::Tick(t) => execute(&mut slow, t.turn),
            _ => {}
        }
    }
    drop(slow);
    fast.join().unwrap();
    server.join().unwrap().unwrap();
    assert!(
        skipped.len() >= 2,
        "expected skipped turns, got {skipped:?}"
    );
    assert_eq!(
        skipped[0], 4,
        "the first skipped turn follows the slow Tick"
    );
    assert!(skipped.windows(2).all(|w| w[1] == w[0] + 1), "{skipped:?}");
}

#[test]
fn deterministic_mode_disables_robots_over_the_hard_limit() {
    let config = ServerConfig {
        rules: small_rules(),
        deterministic: true,
        hard_limit: Duration::from_millis(200),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "a"), entry("b.B", "b")]);
    // B connects but never answers.
    let _silent = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "b");
    let mut a = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "a");
    let mut saw_death = false;
    let mut saw_win = false;
    while let Some(msg) = receive(&mut a) {
        match msg {
            server_message::Msg::RoundStart(_) => execute(&mut a, 0),
            server_message::Msg::Tick(t) => {
                for e in &t.events {
                    match &e.kind {
                        Some(event::Kind::RobotDeath(d)) => saw_death |= d.name == "b.B",
                        Some(event::Kind::Win(_)) => saw_win = true,
                        _ => {}
                    }
                }
                execute(&mut a, t.turn);
            }
            server_message::Msg::BattleOver(_) => break,
            _ => {}
        }
    }
    let outcome = server.join().unwrap().unwrap();
    assert!(saw_death && saw_win, "death {saw_death}, win {saw_win}");
    assert_eq!(outcome.results[0].firsts, 1);
    assert_eq!(outcome.results[1].firsts, 0);
}
