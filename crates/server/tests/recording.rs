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

fn entry(name: &str, token: &str, interactive: bool) -> RobotEntry {
    RobotEntry {
        token: token.into(),
        debug: false,
        statics: v1::RobotStatics {
            name: name.into(),
            full_class_name: name.into(),
            r#type: Some(v1::RobotType {
                advanced: true,
                interactive,
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
/// prints something and sets and removes debug properties now and then. A turn with
/// mouse or keyboard events it prints and fires hard; it returns them, by turn.
fn scripted_robot(
    address: String,
    token: &'static str,
    seed: u64,
) -> thread::JoinHandle<Vec<(i64, v1::InputEvent)>> {
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
        let mut inputs = Vec::new();
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut stream) {
            let mut got_input = false;
            let (turn, paint_enabled) = match message.msg {
                Some(server_message::Msg::RoundStart(_)) => (0, false),
                Some(server_message::Msg::Tick(t)) => {
                    for e in t.events {
                        if let Some(v1::event::Kind::Input(input)) = e.kind {
                            inputs.push((t.turn, input));
                            got_input = true;
                        }
                    }
                    (t.turn, t.paint_enabled)
                }
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
            if got_input {
                bullet_id += 1;
                commands.bullets.push(v1::BulletCommand {
                    bullet_id,
                    power: Some(3.0),
                    ..Default::default()
                });
            }
            let output = if got_input {
                format!("{token} got input at turn {turn}\n")
            } else if turn % 37 == 5 {
                format!("{token} at turn {turn}\n")
            } else {
                String::new()
            };
            let property = |key: &str, value: String| v1::DebugProperty {
                key: key.into(),
                value,
            };
            let debug_properties = match turn % 50 {
                3 | 13 => vec![
                    property("phase", format!("{token} {turn}")),
                    property("energy", format!("{}", next())),
                ],
                // An empty value removes the property.
                20 => vec![property("phase", String::new())],
                _ => Vec::new(),
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
                    debug_properties,
                    ..Default::default()
                })),
            };
            if write_message(&mut stream, &execute).is_err() {
                break;
            }
        }
        inputs
    })
}

/// What the observer does at a (round, turn).
#[derive(Clone, Copy)]
enum Act {
    Stop(i32, i64),
    /// Kills a robot (by index).
    Kill(i32, i64, i32),
    /// Sends these mouse and keyboard events, while paused so that they arrive together.
    Input(i32, i64, &'static [v1::InputEvent]),
}

/// What a fight gives: the outcome, what the observer saw, and the mouse and keyboard
/// events each robot got, by turn.
type Fought = (BattleOutcome, Vec<Frame>, [Vec<(i64, v1::InputEvent)>; 2]);

/// Fights a recorded battle of three rounds between two scripted robots, with robot a's
/// debug paint on (a is interactive, b isn't); an observer acts at the given turn, if any.
fn fight(path: &Path, act: Option<Act>) -> Fought {
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
    let robots = vec![entry("a.A", "a", true), entry("b.B", "b", false)];
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
                    use v1::observer_control::Control;
                    let controls = match act {
                        Some(Act::Stop(round, turn)) if (round, turn) == at(&snapshot) => {
                            vec![Control::Abort(true)]
                        }
                        Some(Act::Kill(round, turn, robot)) if (round, turn) == at(&snapshot) => {
                            vec![Control::Kill(robot)]
                        }
                        Some(Act::Input(round, turn, inputs)) if (round, turn) == at(&snapshot) => {
                            std::iter::once(Control::Pause(true))
                                .chain(inputs.iter().map(|i| Control::Input(*i)))
                                .chain(std::iter::once(Control::Pause(false)))
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    for control in controls {
                        let message = v1::ClientMessage {
                            msg: Some(client_message::Msg::ObserverControl(v1::ObserverControl {
                                control: Some(control),
                            })),
                        };
                        write_message(&mut observer, &message).unwrap();
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
    (outcome, seen, [a.join().unwrap(), b.join().unwrap()])
}

fn at(snapshot: &v1::TurnSnapshot) -> (i32, i64) {
    (snapshot.round, snapshot.turn)
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
    let (outcome, seen, _) = fight(&path, None);
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
    // Debug properties reach the observer (and replay identically, above), with removals.
    let properties = |f: &Frame| match f {
        Frame::Snapshot { snapshot, .. } => snapshot
            .robots
            .iter()
            .flat_map(|r| r.debug_properties.clone())
            .collect(),
        Frame::RoundOver(_) => Vec::new(),
    };
    let all: Vec<v1::DebugProperty> = seen.iter().flat_map(properties).collect();
    assert!(
        all.iter().any(|p| p.key == "phase" && p.value == "a 13"),
        "no debug properties in the battle"
    );
    assert!(
        all.iter().any(|p| p.key == "phase" && p.value.is_empty()),
        "no removed debug property"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stopped_battle_replays_exactly_with_the_results_so_far() {
    let dir = temp_dir("stopped");
    let path = dir.join("battle.rsr");
    let (outcome, seen, _) = fight(&path, Some(Act::Stop(1, 400)));
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

#[test]
fn a_killed_robot_dies_and_the_battle_replays_exactly() {
    let dir = temp_dir("killed");
    let path = dir.join("battle.rsr");
    let (outcome, seen, _) = fight(&path, Some(Act::Kill(0, 50, 1)));
    check_replay(&path, &outcome, &seen);
    assert!(!outcome.aborted);
    assert_eq!(outcome.rounds, 3);
    // Robot b dies after the kill, with an explosion and no energy left.
    let killed = seen.iter().find_map(|f| match f {
        Frame::Snapshot { snapshot, .. }
            if snapshot.round == 0 && snapshot.robots[1].state == v1::RobotState::Dead as i32 =>
        {
            Some(snapshot.clone())
        }
        _ => None,
    });
    let killed = killed.expect("robot b never died in round 1");
    assert!(killed.turn > 50, "died at turn {}", killed.turn);
    assert_eq!(killed.robots[1].energy.unwrap_or_default(), 0.0);
    assert_ne!(killed.robots[0].state, v1::RobotState::Dead as i32);
    assert!(
        killed.bullets.iter().any(|b| b.owner == 1 && b.explosion),
        "no explosion"
    );
    // Without the kill, robot b lived on at that turn: the kill killed it.
    let (_, unkilled, _) = fight(&dir.join("unkilled.rsr"), None);
    let same_turn = unkilled.iter().find_map(|f| match f {
        Frame::Snapshot { snapshot, .. } if at(snapshot) == at(&killed) => Some(snapshot),
        _ => None,
    });
    assert_ne!(
        same_turn.expect("turn not reached").robots[1].state,
        v1::RobotState::Dead as i32
    );
    // Robot a outlived b in the first round: it won it.
    let first_round = seen.iter().find_map(|f| match f {
        Frame::RoundOver(r) => Some(r.results.clone()),
        Frame::Snapshot { .. } => None,
    });
    assert_eq!(first_round.unwrap()[0].firsts, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn mouse(action: v1::MouseAction, x: i32, y: i32) -> v1::InputEvent {
    v1::InputEvent {
        kind: Some(v1::input_event::Kind::Mouse(v1::MouseInput {
            action: action as i32,
            x,
            y,
            when: 1_700_000_000_000,
            ..Default::default()
        })),
    }
}

#[test]
fn input_events_reach_interactive_robots_and_the_battle_replays_exactly() {
    use std::sync::LazyLock;
    static INPUTS: LazyLock<[v1::InputEvent; 4]> = LazyLock::new(|| {
        [
            mouse(v1::MouseAction::MouseMoved, 10, 20),
            // Replaces the move before it: one a turn.
            mouse(v1::MouseAction::MouseMoved, 30, 40),
            mouse(v1::MouseAction::MousePressed, 30, 40),
            v1::InputEvent {
                kind: Some(v1::input_event::Kind::Key(v1::KeyInput {
                    action: v1::KeyAction::KeyPressed as i32,
                    key_code: 38, // VK_UP
                    key_char: 0xFFFF,
                    key_location: 1,
                    ..Default::default()
                })),
            },
        ]
    });
    let dir = temp_dir("input");
    let path = dir.join("battle.rsr");
    let (outcome, seen, [a, b]) = fight(&path, Some(Act::Input(0, 50, &*INPUTS)));
    check_replay(&path, &outcome, &seen);
    assert_eq!(outcome.rounds, 3);
    // Robot a (interactive) got them in one turn after 50, the moves coalesced; b nothing.
    let expected: Vec<v1::InputEvent> = INPUTS[1..].to_vec();
    assert_eq!(a.iter().map(|(_, e)| *e).collect::<Vec<_>>(), expected);
    let turn = a[0].0;
    assert!(turn > 50, "delivered at turn {turn}");
    assert!(a.iter().all(|(t, _)| *t == turn));
    assert!(b.is_empty(), "robot b got {b:?}");
    // They are recorded, in order, before the step that delivered them.
    let recording = Recording::read(&path).unwrap();
    let recorded: Vec<v1::InputEvent> = recording
        .entries
        .iter()
        .filter_map(|e| match e {
            v1::recording_entry::Entry::Input(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(recorded, expected);
    // Robot a reacted: its output says so, in the snapshot of that turn.
    let reacted = seen.iter().any(|f| {
        matches!(f, Frame::Snapshot { snapshot, .. }
        if snapshot.round == 0 && snapshot.turn == turn
            && snapshot.robots[0].output.contains(&format!("a got input at turn {turn}")))
    });
    assert!(reacted, "robot a's reaction isn't in turn {turn}");
    let _ = std::fs::remove_dir_all(&dir);
}
