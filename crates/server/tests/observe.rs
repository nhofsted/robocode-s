// SPDX-License-Identifier: EPL-2.0

//! Observers from elsewhere (design §10): a listener of their own, a token, and the
//! battle from its first turn to its results.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::watch::Watcher;
use robocode_s_server::{
    BattleOutcome, Connection, Observers, RobotEntry, ServerConfig, run_battle_with,
};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

/// The outcome as an observer from elsewhere sees it: without the robots' connections.
fn watched(outcome: BattleOutcome) -> BattleOutcome {
    BattleOutcome {
        connections: Vec::new(),
        threads: Vec::new(),
        ..outcome
    }
}

fn entry(name: &str, token: &str) -> RobotEntry {
    RobotEntry {
        token: token.into(),
        debug: false,
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

/// A robot that answers every turn with no commands; the last turn it got.
fn idle_robot(address: String, token: &'static str) -> thread::JoinHandle<i64> {
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
        let mut last = 0;
        while let Ok(Some(m)) = read_message::<v1::ServerMessage>(&mut s) {
            let turn = match m.msg {
                Some(server_message::Msg::RoundStart(_)) => 0,
                Some(server_message::Msg::Tick(t)) => t.turn,
                _ => continue,
            };
            last = last.max(turn);
            let execute = v1::ClientMessage {
                msg: Some(client_message::Msg::Execute(v1::Execute {
                    turn,
                    commands: Some(v1::RobotCommands::default()),
                    ..Default::default()
                })),
            };
            let _ = write_message(&mut s, &execute);
        }
        last
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
        tls: None,
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
    let refused = Watcher::connect(&observe_address, "guess", None)
        .err()
        .unwrap();
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
    let watcher = Watcher::connect(&observe_address, "s3cret", None).unwrap();
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
    assert_eq!(served.connections, [Connection::Kept; 2]);
    assert_eq!(
        outcome,
        watched(served),
        "the observer gets the battle's results"
    );
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
        tls: None,
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
    let local = Watcher::connect(&robots_address, "", None).unwrap();
    let local = thread::spawn(move || local.run(None, |_| {}));
    let robot = idle_robot(robots_address, "a");
    let watcher = Watcher::connect(&observe_address, "", None).unwrap();
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
            let _ = handle.shutdown();
        }
    });
    let outcome = server.join().unwrap().unwrap();
    assert_eq!(outcome.rounds, 1);
    assert_eq!(local.join().unwrap().unwrap(), watched(outcome));
    robot.join().unwrap();
}

#[test]
fn a_remote_observer_that_stops_reading_does_not_hold_the_battle_up() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let observe_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let observe_address = observe_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 60,
            inactivity_time: 20,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let observers = Observers {
        listener: observe_listener,
        token: String::new(),
        tls: None,
        wait: true,
    };
    let (done, finished) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let outcome = run_battle_with(
            robots_listener,
            Some(observers),
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        );
        let _ = done.send(outcome);
    });
    let robots = [
        idle_robot(robots_address.clone(), "a"),
        idle_robot(robots_address, "b"),
    ];
    // An observer that says hello and then never reads: the server's writes to it fill
    // its socket and then block.
    let mut stalled = TcpStream::connect(&observe_address).unwrap();
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Observer as i32,
            ..Default::default()
        })),
    };
    write_message(&mut stalled, &hello).unwrap();
    let outcome = finished
        .recv_timeout(Duration::from_secs(60))
        .expect("the battle ends although an observer stopped reading")
        .unwrap();
    assert_eq!(outcome.rounds, 60);
    for r in robots {
        r.join().unwrap();
    }
    drop(stalled);
}

/// A robot whose thread ends at once, as one whose `run` returns (sample.SittingDuck)
/// or crashed: alive, but it answers no more turns.
fn stopped_robot(address: String, token: &'static str) -> thread::JoinHandle<()> {
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
            if let Some(server_message::Msg::RoundStart(_)) = m.msg {
                let execute = v1::ClientMessage {
                    msg: Some(client_message::Msg::Execute(v1::Execute {
                        turn: 0,
                        commands: Some(v1::RobotCommands::default()),
                        stopped: true,
                        ..Default::default()
                    })),
                };
                let _ = write_message(&mut s, &execute);
            }
        }
    })
}

#[test]
fn controls_apply_while_no_robot_runs_anymore() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            None,
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        )
    });
    let watcher = Watcher::connect(&robots_address, "", None).unwrap();
    let robots = [
        stopped_robot(robots_address.clone(), "a"),
        stopped_robot(robots_address, "b"),
    ];
    let (tx, rx) = std::sync::mpsc::channel();
    let control = |c| v1::ObserverControl { control: Some(c) };
    use v1::observer_control::Control;
    tx.send(control(Control::TurnsPerSecond(5))).unwrap();
    let handle = watcher.handle().unwrap();
    let mut faster = None;
    let mut fast = None;
    let outcome = watcher.run(Some(rx), |msg| {
        let server_message::Msg::TurnSnapshot(s) = msg else {
            return;
        };
        match s.turn {
            3 if faster.is_none() => {
                // Slow, then as fast as it goes.
                faster = Some(std::time::Instant::now());
                let _ = tx.send(control(Control::TurnsPerSecond(0)));
            }
            100 if fast.is_none() => {
                fast = faster.map(|t| t.elapsed());
                let _ = tx.send(control(Control::Abort(true)));
            }
            _ => {}
        }
        // At 5 turns a second the round would take minutes: give up.
        if faster.is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
            let _ = handle.shutdown();
        }
    });
    let fast = fast.expect("97 turns within 10 s of the speed going to max");
    assert!(fast < Duration::from_secs(3), "97 turns took {fast:?}");
    let outcome = outcome.unwrap();
    assert!(outcome.aborted, "Stop ends the battle");
    assert_eq!(outcome.rounds, 1, "no round starts after the stop");
    assert_eq!(watched(server.join().unwrap().unwrap()), outcome);
    for r in robots {
        r.join().unwrap();
    }
}

#[test]
fn a_faster_rate_applies_without_waiting_out_the_slow_turn() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 1,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            None,
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        )
    });
    let watcher = Watcher::connect(&robots_address, "", None).unwrap();
    let robots = [
        idle_robot(robots_address.clone(), "a"),
        idle_robot(robots_address, "b"),
    ];
    let (tx, rx) = std::sync::mpsc::channel();
    let control = |c| v1::ObserverControl { control: Some(c) };
    use v1::observer_control::Control;
    tx.send(control(Control::TurnsPerSecond(1))).unwrap();
    let (mut faster, mut next) = (None, None);
    let outcome = watcher.run(Some(rx), |msg| {
        let server_message::Msg::TurnSnapshot(s) = msg else {
            return;
        };
        match s.turn {
            // A turn a second; right after a turn, as fast as it goes.
            1 if faster.is_none() => {
                faster = Some(std::time::Instant::now());
                let _ = tx.send(control(Control::TurnsPerSecond(0)));
            }
            2 if next.is_none() => {
                next = faster.map(|t| t.elapsed());
                let _ = tx.send(control(Control::Abort(true)));
            }
            _ => {}
        }
    });
    let next = next.unwrap();
    assert!(
        next < Duration::from_millis(500),
        "the next turn after {next:?}"
    );
    assert!(outcome.unwrap().aborted);
    server.join().unwrap().unwrap();
    for r in robots {
        r.join().unwrap();
    }
}

/// The battle's observer and robots may be silent for longer than a connection has to
/// say hello (10 s): the GUI on another tab sends no controls, robots in a paused battle
/// no `Execute`. On Windows their readers kept the hello's timeout and ended, after which
/// speed, pause and stop went nowhere while the turns still came.
#[test]
fn controls_apply_after_the_observer_and_robots_were_silent_for_long() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 1,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            None,
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        )
    });
    let watcher = Watcher::connect(&robots_address, "", None).unwrap();
    let robots = [
        idle_robot(robots_address.clone(), "a"),
        idle_robot(robots_address, "b"),
    ];
    let (tx, rx) = std::sync::mpsc::channel();
    let control = |c| v1::ObserverControl { control: Some(c) };
    use v1::observer_control::Control;
    let handle = watcher.handle().unwrap();
    let mut waiting = None;
    let outcome = watcher.run(Some(rx), |msg| {
        let server_message::Msg::TurnSnapshot(s) = msg else {
            return;
        };
        match s.turn {
            2 if waiting.is_none() => {
                let _ = tx.send(control(Control::Pause(true)));
                // Nothing from the observer, nor from the paused robots, for 12 s; then
                // on, as fast as it goes.
                let (tx, handle) = (tx.clone(), handle.try_clone().unwrap());
                waiting = Some(thread::spawn(move || {
                    thread::sleep(Duration::from_secs(12));
                    let _ = tx.send(control(Control::TurnsPerSecond(0)));
                    let _ = tx.send(control(Control::Pause(false)));
                    // Unheard: give up.
                    thread::sleep(Duration::from_secs(20));
                    let _ = handle.shutdown();
                }));
            }
            50 => {
                let _ = tx.send(control(Control::Abort(true)));
            }
            _ => {}
        }
    });
    let outcome = outcome.expect("the battle went on and stopped as told");
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 1);
    server.join().unwrap().unwrap();
    for r in robots {
        let last = r.join().unwrap();
        assert!(last >= 50, "a robot got turns up to {last} only");
    }
    drop(waiting);
}

/// The battle's own observer (the GUI's) losing its connection stops the battle: nobody
/// would see or control it anymore.
#[test]
fn the_battle_stops_when_its_own_observer_is_gone() {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            None,
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        )
    });
    let watcher = Watcher::connect(&robots_address, "", None).unwrap();
    let robots = [
        idle_robot(robots_address.clone(), "a"),
        idle_robot(robots_address, "b"),
    ];
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(v1::ObserverControl {
        control: Some(v1::observer_control::Control::TurnsPerSecond(50)),
    })
    .unwrap();
    let handle = watcher.handle().unwrap();
    let watched = watcher.run(Some(rx), |msg| {
        if let server_message::Msg::TurnSnapshot(s) = msg
            && s.turn == 5
        {
            let _ = handle.shutdown();
        }
    });
    assert!(watched.is_err(), "the connection broke");
    let outcome = server.join().unwrap().unwrap();
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 1);
    for r in robots {
        r.join().unwrap();
    }
}

/// A battle of two idle robots, three rounds with a short inactivity time, watched by an
/// observer on the robots' listener that sends `act`'s controls; its outcome.
fn stopped_by(
    mut act: impl FnMut(&server_message::Msg) -> Vec<v1::observer_control::Control>,
) -> robocode_s_server::BattleOutcome {
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            inactivity_time: 20,
            ..Default::default()
        },
        deterministic: true,
        ..Default::default()
    };
    let server = thread::spawn(move || {
        run_battle_with(
            robots_listener,
            None,
            &config,
            vec![entry("a.A", "a"), entry("b.B", "b")],
        )
    });
    let watcher = Watcher::connect(&robots_address, "", None).unwrap();
    let robots = [
        idle_robot(robots_address.clone(), "a"),
        idle_robot(robots_address, "b"),
    ];
    let (tx, rx) = std::sync::mpsc::channel();
    let outcome = watcher
        .run(Some(rx), |msg| {
            for control in act(msg) {
                let _ = tx.send(v1::ObserverControl {
                    control: Some(control),
                });
            }
        })
        .unwrap();
    assert_eq!(watched(server.join().unwrap().unwrap()), outcome);
    for r in robots {
        r.join().unwrap();
    }
    outcome
}

#[test]
fn a_stop_while_paused_ends_the_battle_in_its_round() {
    use v1::observer_control::Control;
    let outcome = stopped_by(|msg| match msg {
        server_message::Msg::TurnSnapshot(s) if s.round == 0 && s.turn == 5 => {
            vec![Control::Pause(true)]
        }
        server_message::Msg::TurnSnapshot(s) if s.round == 0 && s.turn == 6 => {
            // The turn that ran before the pause came; stop once it holds.
            thread::sleep(Duration::from_millis(200));
            vec![Control::Abort(true)]
        }
        _ => Vec::new(),
    });
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 1);
}

#[test]
fn a_stop_at_the_end_of_a_round_starts_no_more_than_the_next() {
    use v1::observer_control::Control;
    let outcome = stopped_by(|msg| match msg {
        server_message::Msg::RoundResult(_) => vec![Control::Abort(true)],
        _ => Vec::new(),
    });
    assert!(outcome.aborted);
    assert!(outcome.rounds <= 2, "{} rounds", outcome.rounds);
}

#[test]
fn observers_from_elsewhere_watch_over_tls_with_the_pinned_certificate() {
    use robocode_s_server::tls::{Fingerprint, Identity};
    let dir = std::env::temp_dir().join(format!("robocodes-observe-tls-{}", std::process::id()));
    let identity = Identity::load_or_create(&dir).unwrap();
    let pin = identity.fingerprint;
    let robots_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let robots_address = robots_listener.local_addr().unwrap().to_string();
    // On loopback, but with TLS, as `--observe PORT --tls` has it.
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
        token: "s3cret".into(),
        tls: Some(identity),
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
    let wrong = Watcher::connect(&observe_address, "s3cret", Some(Fingerprint([9; 32])))
        .err()
        .unwrap();
    assert!(wrong.contains("not the pinned"), "{wrong}");
    let plain = Watcher::connect(&observe_address, "s3cret", None)
        .err()
        .unwrap();
    assert!(plain.contains("if it wants TLS"), "{plain}");
    let watcher = Watcher::connect(&observe_address, "s3cret", Some(pin)).unwrap();
    let outcome = watcher.run(None, |_| {}).unwrap();
    assert_eq!(outcome, watched(server.join().unwrap().unwrap()));
    for r in robots {
        r.join().unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plain_observers_listener_off_loopback_is_refused() {
    let Ok(open) = TcpListener::bind("0.0.0.0:0") else {
        return;
    };
    let observers = Observers {
        listener: open,
        token: "t".into(),
        tls: None,
        wait: false,
    };
    let e = run_battle_with(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        Some(observers),
        &ServerConfig::default(),
        vec![],
    )
    .unwrap_err();
    assert!(e.to_string().contains("needs TLS"), "{e}");
}
