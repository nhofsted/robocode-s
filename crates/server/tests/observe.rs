// SPDX-License-Identifier: EPL-2.0

//! Observers from elsewhere (design §10): a listener of their own, a token, and the
//! battle from its first turn to its results.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::watch::Watcher;
use robocode_s_server::{Observers, RobotEntry, ServerConfig, run_battle_with};
use std::net::{TcpListener, TcpStream};
use std::thread;
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

/// A robot that answers every turn with no commands.
fn idle_robot(address: String, token: &'static str) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut s = TcpStream::connect(&address).unwrap();
        let hello = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(v1::Hello {
                version: Some(PROTOCOL_VERSION),
                role: v1::Role::Robot as i32,
                token: token.into(),
                ..Default::default()
            })),
        };
        write_message(&mut s, &hello).unwrap();
        while let Ok(Some(m)) = read_message::<v1::ServerMessage>(&mut s) {
            let turn = match m.msg {
                Some(server_message::Msg::RoundStart(_)) => 0,
                Some(server_message::Msg::Tick(t)) => t.turn,
                _ => continue,
            };
            let execute = v1::ClientMessage {
                msg: Some(client_message::Msg::Execute(v1::Execute {
                    turn,
                    commands: Some(v1::RobotCommands::default()),
                    ..Default::default()
                })),
            };
            let _ = write_message(&mut s, &execute);
        }
    })
}

#[test]
fn a_remote_observer_with_the_token_watches_the_battle_to_its_results() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let observe_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let observe_address = observe_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 2,
            inactivity_time: 20,
            ..Default::default()
        },
        seed: -42,
        deterministic: true,
        ..Default::default()
    };
    let observers = Observers {
        listener: observe_listener,
        token: "s3cret".into(),
        wait: true,
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            Some(observers),
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        )
    });
    let robots = [
        idle_robot(robots_address.clone(), "a"),
        idle_robot(robots_address, "b"),
    ];

    // The wrong token, and robots, are turned away.
    let refused = Watcher::connect(&observe_address, "guess").err().unwrap();
    assert!(refused.contains("wrong observer token"), "{refused}");
    let mut robot = TcpStream::connect(&observe_address).unwrap();
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Robot as i32,
            token: "a".into(),
            ..Default::default()
        })),
    };
    write_message(&mut robot, &hello).unwrap();
    assert!(matches!(
        read_message::<v1::ServerMessage>(&mut robot).unwrap().and_then(|m| m.msg),
        Some(server_message::Msg::Error(e)) if e.message.contains("only observers")
    ));

    // The battle waits for its observer, which sees it from the first turn.
    thread::sleep(Duration::from_millis(200));
    let watcher = Watcher::connect(&observe_address, "s3cret").unwrap();
    let (mut info, mut snapshots, mut round_results) = (None, Vec::new(), 0);
    let outcome = watcher
        .run(None, |msg| match msg {
            server_message::Msg::BattleInfo(i) => info = Some(i.clone()),
            server_message::Msg::TurnSnapshot(s) => snapshots.push((s.round, s.turn)),
            server_message::Msg::RoundResult(_) => round_results += 1,
            _ => {}
        })
        .unwrap();
    let info = info.expect("BattleInfo first");
    assert_eq!(info.seed, -42);
    assert_eq!(info.robots.len(), 2);
    assert_eq!(info.rules.unwrap().num_rounds, 2);
    assert_eq!(snapshots.first(), Some(&(0, 0)), "from the first turn");
    assert_eq!(snapshots.last().map(|s| s.0), Some(1));
    assert_eq!(round_results, 2);

    let served = server.join().unwrap().unwrap();
    assert_eq!(outcome, served, "the observer gets the battle's results");
    assert_eq!(outcome.rounds, 2);
    assert!(!outcome.aborted);
    for r in robots {
        r.join().unwrap();
    }
}

#[test]
fn a_remote_observer_that_pauses_and_leaves_does_not_hold_the_battle_up() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let observe_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let observe_address = observe_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 1,
            inactivity_time: 20,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let observers = Observers {
        listener: observe_listener,
        token: String::new(),
        wait: true,
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            Some(observers),
            &config,
            vec![entry("a.A", "a")],
        )
    });
    // The command line's own observer, on the robots' listener, stays to the end.
    let local = Watcher::connect(&robots_address, "").unwrap();
    let local = thread::spawn(move || local.run(None, |_| {}));
    let robot = idle_robot(robots_address, "a");
    let watcher = Watcher::connect(&observe_address, "").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = watcher.handle().unwrap();
    let _ = watcher.run(Some(rx), |msg| {
        if let server_message::Msg::TurnSnapshot(s) = msg
            && s.turn == 5
        {
            let _ = tx.send(v1::ObserverControl {
                control: Some(v1::observer_control::Control::Pause(true)),
            });
            thread::sleep(Duration::from_millis(100));
            let _ = handle.shutdown(std::net::Shutdown::Both);
        }
    });
    let outcome = server.join().unwrap().unwrap();
    assert_eq!(outcome.rounds, 1);
    assert_eq!(local.join().unwrap().unwrap(), outcome);
    robot.join().unwrap();
}
