// SPDX-License-Identifier: EPL-2.0

//! A recorded battle replays to exactly what observers saw (design §11).

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::recording::{Frame, Recording};
use robocode_s_server::{BattleOutcome, RobotEntry, ServerConfig, run_battle};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
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
            let (turn, paint_enabled) = match message.msg {
                Some(server_message::Msg::RoundStart(_)) => (0, false),
                Some(server_message::Msg::Tick(t)) => (t.turn, t.paint_enabled),
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
                    // Debug paint only while the server asks for it.
                    paint: if paint_enabled {
                        vec![
                            v1::PaintOp {
                                op: Some(v1::paint_op::Op::SetColor(0x7fff_0000)),
                            },
                            v1::PaintOp {
                                op: Some(v1::paint_op::Op::Draw(v1::Shape {
                                    shape: Some(v1::shape::Shape::Line(v1::Line {
                                        x1: Some(0.0),
                                        y1: Some(0.0),
                                        x2: Some(turn as f64),
                                        y2: Some(next() * 400.0),
                                    })),
                                })),
                            },
                        ]
                    } else {
                        Vec::new()
                    },
                    // CPU time: timing, recorded for replays but not hashed.
                    cpu_nanos: 1_000 + turn as u64,
                    ..Default::default()
                })),
            };
            if write_message(&mut stream, &execute).is_err() {
                break;
            }
        }
    })
}

/// Fights a recorded battle of three rounds between two scripted robots, with robot a's
/// debug paint on; an observer stops it at `stop` (round, turn) if given. Returns the
/// outcome and what the observer saw.
fn fight(path: &Path, stop: Option<(i32, i64)>) -> (BattleOutcome, Vec<Frame>) {
    let path = path.to_path_buf();
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            width: 400,
            height: 400,
            ..Default::default()
        },
        seed: 7,
        deterministic: true,
        record: Some(path),
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let robots = vec![entry("a.A", "a"), entry("b.B", "b")];
    let server = thread::spawn(move || run_battle(listener, &config, robots));

    let mut observer = connect(&address, v1::Role::Observer, "");
    // Robot a paints; b doesn't.
    let toggle = v1::ClientMessage {
        msg: Some(client_message::Msg::ObserverControl(v1::ObserverControl {
            control: Some(v1::observer_control::Control::Paint(v1::PaintToggle {
                robot_index: 0,
                enabled: true,
            })),
        })),
    };
    write_message(&mut observer, &toggle).unwrap();
    let watcher = thread::spawn(move || {
        let mut frames = Vec::new();
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut observer) {
            match message.msg {
                Some(server_message::Msg::TurnSnapshot(snapshot)) => {
                    if stop == Some((snapshot.round, snapshot.turn)) {
                        let abort = v1::ClientMessage {
                            msg: Some(client_message::Msg::ObserverControl(v1::ObserverControl {
                                control: Some(v1::observer_control::Control::Abort(true)),
                            })),
                        };
                        write_message(&mut observer, &abort).unwrap();
                    }
                    frames.push(Frame::Snapshot {
                        snapshot,
                        verified: true,
                    });
                }
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
    (outcome, seen)
}

/// Replays the recording at `path`: it must give exactly the frames `seen`, and end with
/// the battle's results.
fn check_replay(path: &Path, outcome: &BattleOutcome, seen: &[Frame]) {
    let recording = Recording::read(path).unwrap();
    assert_eq!(recording.header.seed, 7);
    assert_eq!(recording.header.robots.len(), 2);
    let mut replay = recording.replay();
    let replayed: Vec<Frame> = replay.by_ref().collect();
    assert!(replay.is_done());
    assert_eq!(replayed.len(), seen.len());
    for (i, (r, s)) in replayed.iter().zip(seen).enumerate() {
        assert_eq!(r, s, "frame {i} differs");
    }
    let final_results = robocode_s_server::recording::round_result(replay.battle()).results;
    assert_eq!(final_results, outcome.results);
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("robocodes-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_recorded_battle_replays_exactly() {
    let dir = temp_dir("recording");
    let path = dir.join("battle.rsr");
    let (outcome, seen) = fight(&path, None);
    check_replay(&path, &outcome, &seen);
    assert!(!outcome.aborted);
    assert_eq!(outcome.rounds, 3);
    assert!(seen.len() > 100, "{} frames seen", seen.len());
    assert!(
        seen.iter()
            .any(|f| matches!(f, Frame::Snapshot { snapshot, .. }
            if snapshot.robots.iter().any(|r| !r.output.is_empty()))),
        "no console output in the battle"
    );
    let painted = |f: &Frame, i: usize| matches!(f, Frame::Snapshot { snapshot, .. } if !snapshot.robots[i].paint.is_empty());
    assert!(seen.iter().any(|f| painted(f, 0)), "robot a never painted");
    assert!(
        seen.iter()
            .any(|f| matches!(f, Frame::Snapshot { snapshot, .. }
            if snapshot.robots.iter().all(|r| r.cpu_nanos > 1_000))),
        "no CPU times in the battle"
    );
    assert!(
        !seen.iter().any(|f| painted(f, 1)),
        "robot b painted unasked"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stopped_battle_replays_exactly_with_the_results_so_far() {
    let dir = temp_dir("stopped");
    let path = dir.join("battle.rsr");
    let (outcome, seen) = fight(&path, Some((1, 400)));
    check_replay(&path, &outcome, &seen);
    assert!(outcome.aborted);
    assert_eq!(outcome.rounds, 2);
    let last = seen.iter().rev().find_map(|f| match f {
        Frame::Snapshot { snapshot, .. } => Some((snapshot.round, snapshot.turn)),
        Frame::RoundOver(_) => None,
    });
    assert!(matches!(last, Some((1, turn)) if turn > 400), "{last:?}");
    // Ranked, and the stopped round counts: the scores are more than the first round's.
    let first_round = seen.iter().find_map(|f| match f {
        Frame::RoundOver(r) => Some(r.results.clone()),
        Frame::Snapshot { .. } => None,
    });
    let first_round = first_round.unwrap();
    let mut ranks: Vec<i32> = outcome.results.iter().map(|r| r.rank).collect();
    ranks.sort();
    assert_eq!(ranks, [1, 2]);
    let score = |r: &v1::Results| r.score.unwrap_or_default();
    let total = |rs: &[v1::Results]| rs.iter().map(score).sum::<f64>();
    assert!(
        total(&outcome.results) > total(&first_round),
        "{:?} {first_round:?}",
        outcome.results
    );
    let best = outcome.results.iter().find(|r| r.rank == 1).unwrap();
    assert!(outcome.results.iter().all(|r| score(r) <= score(best)));
    let _ = std::fs::remove_dir_all(&dir);
}
