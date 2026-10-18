// SPDX-License-Identifier: EPL-2.0

//! The server's connection handling and turn flow with small scripted robots.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, event, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::{
    BattleOutcome, DETERMINISTIC_BUDGET_NANOS, RobotEntry, ServerConfig, late_turns, run_battle,
};
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

fn execute_with_work(stream: &mut TcpStream, turn: i64, cpu_nanos: u64) {
    let message = v1::ClientMessage {
        msg: Some(client_message::Msg::Execute(v1::Execute {
            turn,
            cpu_nanos,
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

/// A deterministic two-robot battle in which `b.B` reports `work` for its turn `at` (and
/// nothing otherwise). Returns what B saw: the turns of its Ticks, its SkippedTurn events,
/// and whether a RobotDeath for B reached A.
fn work_battle(at: i64, work: u64) -> (Vec<i64>, Vec<i64>, bool) {
    let config = ServerConfig {
        rules: BattleRules {
            inactivity_time: 400,
            ..small_rules()
        },
        deterministic: true,
        hard_limit: Duration::from_secs(10),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "a"), entry("b.B", "b")]);
    let a = {
        let address = address.clone();
        thread::spawn(move || {
            let mut s = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "a");
            let mut saw_b_death = false;
            while let Some(msg) = receive(&mut s) {
                match msg {
                    server_message::Msg::RoundStart(_) => execute(&mut s, 0),
                    server_message::Msg::Tick(t) => {
                        assert_eq!(t.budget_nanos, DETERMINISTIC_BUDGET_NANOS);
                        for e in &t.events {
                            if let Some(event::Kind::RobotDeath(d)) = &e.kind {
                                saw_b_death |= d.name == "b.B";
                            }
                        }
                        execute(&mut s, t.turn);
                    }
                    server_message::Msg::BattleOver(_) => break,
                    _ => {}
                }
            }
            saw_b_death
        })
    };
    let mut b = connect(&address, PROTOCOL_VERSION.major, v1::Role::Robot, "b");
    let (mut ticks, mut skipped) = (Vec::new(), Vec::new());
    while let Some(msg) = receive(&mut b) {
        match msg {
            server_message::Msg::RoundStart(_) => execute(&mut b, 0),
            server_message::Msg::Tick(t) => {
                assert_eq!(t.budget_nanos, DETERMINISTIC_BUDGET_NANOS);
                ticks.push(t.turn);
                for e in &t.events {
                    if let Some(event::Kind::SkippedTurn(s)) = &e.kind {
                        skipped.push(s.turn);
                    }
                }
                let work = if t.turn == at { work } else { 0 };
                execute_with_work(&mut b, t.turn, work);
            }
            server_message::Msg::BattleOver(_) => break,
            _ => {}
        }
    }
    let saw_b_death = a.join().unwrap();
    server.join().unwrap().unwrap();
    (ticks, skipped, saw_b_death)
}

#[test]
fn late_turns_counts_the_turns_a_robot_overran() {
    let b = DETERMINISTIC_BUDGET_NANOS;
    // Within the budget: on time.
    assert_eq!(late_turns(0, 5, b), 0);
    assert_eq!(late_turns(b - 1, 5, b), 0);
    // At the budget: one turn late; then one more per further budget.
    assert_eq!(late_turns(b, 5, b), 1);
    assert_eq!(late_turns(2 * b - 1, 5, b), 1);
    assert_eq!(late_turns(2 * b, 5, b), 2);
    assert_eq!(late_turns(3 * b + 1, 5, b), 3);
    assert_eq!(late_turns(40 * b, 5, b), 40);
    // Turn 1 allows seven budgets.
    assert_eq!(late_turns(7 * b - 1, 1, b), 0);
    assert_eq!(late_turns(7 * b, 1, b), 1);
    assert_eq!(late_turns(9 * b, 1, b), 3);
    // Turn 0 (the round's start) is never late.
    assert_eq!(late_turns(0, 0, b), 0);
    assert_eq!(late_turns(1000 * b, 0, b), 0);
}

#[test]
fn deterministic_mode_skips_turns_by_counted_work() {
    let b = DETERMINISTIC_BUDGET_NANOS;
    let (at, work) = (5, 3 * b + 1);
    let late = late_turns(work, at, b);
    assert_eq!(late, 3);
    let (ticks, skipped, saw_death) = work_battle(at, work);
    assert!(!saw_death, "3 skipped turns do not disable a robot");
    let i = ticks.iter().position(|&t| t == at).expect("the slow turn");
    assert_eq!(ticks[i + 1], at + late + 1, "ticks {ticks:?}");
    assert_eq!(skipped, (at + 1..=at + late).collect::<Vec<_>>());
}

#[test]
fn deterministic_mode_disables_robots_that_overrun_too_many_turns() {
    let b = DETERMINISTIC_BUDGET_NANOS;
    let (ticks, _, saw_death) = work_battle(5, 40 * b);
    assert!(saw_death, "B should be disabled; ticks {ticks:?}");
    // B's Execute for turn 5 counts as 40 turns late: the Tick after it is turn 46 (the
    // one that reports its death) and there is none after that.
    assert_eq!(ticks.last(), Some(&46), "ticks {ticks:?}");
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
fn a_battle_stopped_before_its_first_round_leaves_no_recording() {
    let path = std::env::temp_dir().join(format!("robocodes-cancelled-{}.rec", std::process::id()));
    let config = ServerConfig {
        rules: small_rules(),
        connect_timeout: Duration::from_secs(30),
        record: Some(path.clone()),
        ..Default::default()
    };
    let (address, server) = start(config, vec![entry("a.A", "a")]);
    let mut observer = connect(&address, PROTOCOL_VERSION.major, v1::Role::Observer, "");
    // The file is there while the battle starts (a bad path fails at once).
    thread::sleep(Duration::from_millis(100));
    assert!(path.exists());
    control(&mut observer, v1::observer_control::Control::Abort(true));
    let outcome = server.join().unwrap().unwrap();
    assert_eq!(outcome.rounds, 0);
    assert!(
        !path.exists(),
        "no recording of a battle that never started"
    );
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
