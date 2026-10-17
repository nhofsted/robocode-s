// SPDX-License-Identifier: EPL-2.0

//! The server's connection handling and turn flow with small scripted robots.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, event, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::{BattleOutcome, RobotEntry, ServerConfig, run_battle};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
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
    // A answers at once and reports how far the battle got; later it just disconnects and
    // the battle continues with its commands.
    let progress = Arc::new(AtomicI64::new(0));
    let fast = {
        let (address, progress) = (address.clone(), progress.clone());
        thread::spawn(move || {
            let mut s = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "a");
            while let Some(msg) = receive(&mut s) {
                match msg {
                    server_message::Msg::RoundStart(_) => execute(&mut s, 0),
                    server_message::Msg::Tick(t) if t.turn >= 20 => return,
                    server_message::Msg::Tick(t) => {
                        progress.store(t.turn, Ordering::SeqCst);
                        execute(&mut s, t.turn);
                    }
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
                // Too slow: B answers only after the battle reached turn 8 without it.
                while progress.load(Ordering::SeqCst) < 8 {
                    thread::sleep(Duration::from_millis(1));
                }
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
        skipped.len() >= 5,
        "expected turns 4..8 or more to be skipped, got {skipped:?}"
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

fn control(observer: &mut TcpStream, control: v1::observer_control::Control) {
    let message = v1::ClientMessage {
        msg: Some(client_message::Msg::ObserverControl(v1::ObserverControl {
            control: Some(control),
        })),
    };
    write_message(observer, &message).unwrap();
}

#[test]
fn an_observer_stops_the_battle_with_the_results_so_far() {
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 5,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "a"), entry("b.B", "b")]);
    let mut observer = connect(&address, PROTOCOL_VERSION.major, v1::Role::Observer, "");
    let robots: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|token| {
            let address = address.clone();
            thread::spawn(move || {
                let mut s = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, token);
                let (mut ended, mut over, mut should_wait) = (None, None, false);
                while let Some(msg) = receive(&mut s) {
                    match msg {
                        server_message::Msg::RoundStart(_) => execute(&mut s, 0),
                        server_message::Msg::Tick(t) => {
                            should_wait |= t.should_wait;
                            for e in &t.events {
                                if let Some(event::Kind::BattleEnded(b)) = &e.kind {
                                    ended = Some(b.aborted);
                                }
                            }
                            execute(&mut s, t.turn);
                        }
                        server_message::Msg::BattleOver(b) => over = Some(b.aborted),
                        _ => {}
                    }
                }
                (ended, over, should_wait)
            })
        })
        .collect();
    // Pause in the second round, then stop: the pause doesn't hold the stop up.
    let mut last = (0, 0);
    let mut stopped = false;
    while let Some(msg) = receive(&mut observer) {
        if let server_message::Msg::TurnSnapshot(s) = msg {
            if s.round == 1 && s.turn == 10 && !stopped {
                control(&mut observer, v1::observer_control::Control::Pause(true));
                thread::sleep(Duration::from_millis(100));
                control(&mut observer, v1::observer_control::Control::Abort(true));
                stopped = true;
            }
            last = (s.round, s.turn);
        }
    }
    let outcome = server.join().unwrap().unwrap();
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 2);
    // The stopped round runs out (151 turns after the stop at most), but no other starts.
    assert_eq!(last.0, 1);
    assert!(last.1 > 10 && last.1 <= 10 + 152, "last turn {last:?}");
    let mut ranks: Vec<i32> = outcome.results.iter().map(|r| r.rank).collect();
    ranks.sort();
    assert_eq!(ranks, [1, 2]);
    for robot in robots {
        let (ended, over, should_wait) = robot.join().unwrap();
        assert_eq!(ended, Some(true), "BattleEnded(aborted) for every robot");
        assert_eq!(over, Some(true), "BattleOver(aborted) for every robot");
        assert!(should_wait);
    }
}

#[test]
fn a_stop_before_the_robots_connect_ends_the_battle() {
    let config = ServerConfig {
        rules: small_rules(),
        connect_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "a")]);
    let mut observer = connect(&address, PROTOCOL_VERSION.major, v1::Role::Observer, "");
    control(&mut observer, v1::observer_control::Control::Abort(true));
    let outcome = server.join().unwrap().unwrap();
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 0);
}

#[test]
fn the_robots_listener_wants_the_tokens() {
    let config = ServerConfig {
        rules: small_rules(),
        connect_timeout: Duration::from_secs(30),
        observer_token: "observer-secret".into(),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "robot-secret")]);
    // Neither an observer without the token nor a robot with a wrong one gets in.
    for (role, token) in [
        (v1::Role::Observer, ""),
        (v1::Role::Observer, "observer-secreT"),
        (v1::Role::Robot, "robot-0"),
    ] {
        let mut stream = connect(&address, PROTOCOL_VERSION.major, role, token);
        assert!(
            matches!(receive(&mut stream), Some(server_message::Msg::Error(_))),
            "{role:?} with {token:?} refused"
        );
        let _ = write_message(
            &mut stream,
            &v1::ClientMessage {
                msg: Some(client_message::Msg::ObserverControl(v1::ObserverControl {
                    control: Some(v1::observer_control::Control::Abort(true)),
                })),
            },
        );
    }
    thread::sleep(Duration::from_millis(200));
    assert!(!server.is_finished(), "a refused observer stops nothing");
    let mut observer = connect(
        &address,
        PROTOCOL_VERSION.major,
        v1::Role::Observer,
        "observer-secret",
    );
    control(&mut observer, v1::observer_control::Control::Abort(true));
    let outcome = server.join().unwrap().unwrap();
    assert!(outcome.aborted);
}
