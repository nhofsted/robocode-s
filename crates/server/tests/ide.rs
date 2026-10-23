// SPDX-License-Identifier: EPL-2.0

//! Robots from an IDE (#17): the listener, and battles that wait for such a robot as long
//! as it takes.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, event, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::ide::{self, IdeListener};
use robocode_s_server::{BattleOutcome, RobotEntry, ServerConfig, run_battle, run_battle_piped};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn statics(name: &str) -> v1::RobotStatics {
    v1::RobotStatics {
        name: name.into(),
        full_class_name: name.into(),
        r#type: Some(v1::RobotType {
            advanced: true,
            ..Default::default()
        }),
        team_index: -1,
        ..Default::default()
    }
}

fn entry(name: &str, token: &str, debug: bool) -> RobotEntry {
    RobotEntry {
        token: token.into(),
        debug,
        statics: statics(name),
    }
}

fn description(name: &str) -> v1::RobotDescription {
    v1::RobotDescription {
        name: name.into(),
        full_class_name: name.into(),
        r#type: Some(v1::RobotType {
            advanced: true,
            ..Default::default()
        }),
        platform: "JVM test".into(),
        ..Default::default()
    }
}

fn hello(
    role: v1::Role,
    token: &str,
    description: Option<v1::RobotDescription>,
) -> v1::ClientMessage {
    v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: role as i32,
            token: token.into(),
            robot_description: description,
            ..Default::default()
        })),
    }
}

fn robot_hello(token: &str, description: Option<v1::RobotDescription>) -> v1::ClientMessage {
    hello(v1::Role::Robot, token, description)
}

fn receive(stream: &mut TcpStream) -> Option<server_message::Msg> {
    read_message::<v1::ServerMessage>(stream)
        .ok()
        .flatten()
        .and_then(|m| m.msg)
}

fn execute(stream: &mut TcpStream, turn: i64, cpu_nanos: u64) {
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

fn rules() -> BattleRules {
    BattleRules {
        num_rounds: 1,
        inactivity_time: 5,
        ..Default::default()
    }
}

fn listener() -> IdeListener {
    IdeListener::bind("127.0.0.1:0", "secret".into()).unwrap()
}

fn connect(ide: &IdeListener, hello: &v1::ClientMessage) -> TcpStream {
    let mut stream = TcpStream::connect(ide.local_addr()).unwrap();
    write_message(&mut stream, hello).unwrap();
    stream
}

fn refused(stream: &mut TcpStream) -> String {
    match receive(stream) {
        Some(server_message::Msg::Error(e)) => e.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn wait_for(ide: &IdeListener, count: usize) -> Vec<ide::IdeRobot> {
    ide.wait_for(count, &AtomicBool::new(false)).unwrap()
}

#[test]
fn a_robot_describes_itself_and_waits() {
    let ide = listener();
    let _robot = connect(&ide, &robot_hello("secret", Some(description("my.Robot*"))));
    let robots = wait_for(&ide, 1);
    assert_eq!(robots[0].description.name, "my.Robot*");
    assert_eq!(robots[0].description.platform, "JVM test");
}

#[test]
fn refuses_wrong_tokens_teams_and_missing_descriptions() {
    let ide = listener();
    let mut wrong = connect(&ide, &robot_hello("nope", Some(description("a.A"))));
    assert!(refused(&mut wrong).contains("token"));
    let mut none = connect(&ide, &robot_hello("secret", None));
    assert!(refused(&mut none).contains("describes itself"));
    let mut team = description("sampleteam.MyFirstTeam");
    team.team_id = Some("sampleteam.MyFirstTeam[   0]".into());
    let mut refused_team = connect(&ide, &robot_hello("secret", Some(team)));
    assert!(refused(&mut refused_team).contains("teams"));
    let observer = hello(v1::Role::Observer, "secret", Some(description("a.A")));
    let mut wrong_role = connect(&ide, &observer);
    assert!(refused(&mut wrong_role).contains("robots"));
    assert!(ide.robots().is_empty());
}

#[test]
fn a_robot_that_leaves_is_not_offered_anymore() {
    let ide = listener();
    let robot = connect(&ide, &robot_hello("secret", Some(description("a.A"))));
    let id = wait_for(&ide, 1)[0].id;
    drop(robot);
    let until = Instant::now() + Duration::from_secs(5);
    while !ide.robots().is_empty() {
        assert!(Instant::now() < until, "still offered");
        ide.wait_change(Duration::from_millis(50));
    }
    assert!(ide.take(id, 0).is_err());
}

/// A robot that plays along, answering the Tick of `slow_turn` after `delay` with `work`
/// nanoseconds of counted work in every turn; it returns the last turn it saw and the
/// skipped turns it was told about.
fn play(
    mut stream: TcpStream,
    slow_turn: i64,
    delay: Duration,
    work: u64,
) -> JoinHandle<(i64, Vec<i64>)> {
    thread::spawn(move || {
        let (mut turns, mut skipped) = (0, Vec::new());
        while let Some(msg) = receive(&mut stream) {
            match msg {
                server_message::Msg::RoundStart(_) => execute(&mut stream, 0, 0),
                server_message::Msg::Tick(t) => {
                    for e in &t.events {
                        if let Some(event::Kind::SkippedTurn(s)) = &e.kind {
                            skipped.push(s.turn);
                        }
                    }
                    turns = t.turn;
                    if t.turn == slow_turn {
                        thread::sleep(delay);
                    }
                    execute(&mut stream, t.turn, work);
                }
                _ => {}
            }
        }
        (turns, skipped)
    })
}

/// A battle of `a.A` (fast) against the robot from the IDE, which answers turn 3 late.
fn debug_battle(
    config: ServerConfig,
    delay: Duration,
    work: u64,
) -> (BattleOutcome, (i64, Vec<i64>)) {
    let ide = listener();
    let robot = connect(&ide, &robot_hello("secret", Some(description("my.Robot*"))));
    let id = wait_for(&ide, 1)[0].id;
    let pipe = ide.take(id, 1).unwrap();
    let main = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = main.local_addr().unwrap().to_string();
    let robots = vec![entry("a.A", "a", false), entry("my.Robot*", "", true)];
    let server = thread::spawn(move || run_battle_piped(main, None, &config, robots, vec![pipe]));
    let mut fast = TcpStream::connect(&address).unwrap();
    write_message(&mut fast, &robot_hello("a", None)).unwrap();
    let fast = play(fast, -1, Duration::ZERO, 0);
    let ide_robot = play(robot, 3, delay, work);
    let outcome = server.join().unwrap().unwrap();
    fast.join().unwrap();
    (outcome, ide_robot.join().unwrap())
}

#[test]
fn classic_mode_waits_for_a_robot_from_the_ide() {
    let config = ServerConfig {
        rules: rules(),
        turn_timeout: Duration::from_millis(20),
        ..Default::default()
    };
    // Twenty times the turn deadline.
    let (outcome, (turns, skipped)) = debug_battle(config, Duration::from_millis(400), 0);
    assert_eq!(outcome.results.len(), 2);
    assert!(turns > 5, "the battle went on ({turns} turns)");
    assert!(skipped.is_empty(), "no skipped turns: {skipped:?}");
}

#[test]
fn deterministic_mode_neither_disables_nor_skips_a_robot_from_the_ide() {
    let config = ServerConfig {
        rules: rules(),
        deterministic: true,
        hard_limit: Duration::from_millis(50),
        ..Default::default()
    };
    // Eight times the hard limit to answer, and counted work that would make another robot
    // many turns late.
    let (outcome, (turns, skipped)) = debug_battle(config, Duration::from_millis(400), 100_000_000);
    assert_eq!(outcome.results.len(), 2);
    assert!(turns > 5, "the battle went on ({turns} turns)");
    assert!(skipped.is_empty(), "no skipped turns: {skipped:?}");
}

#[test]
fn a_robot_from_the_ide_may_connect_long_after_the_connect_timeout() {
    let config = ServerConfig {
        rules: rules(),
        connect_timeout: Duration::from_millis(100),
        ..Default::default()
    };
    let main = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = main.local_addr().unwrap().to_string();
    let robots = vec![entry("a.A", "a", false), entry("b.B", "b", true)];
    let server = thread::spawn(move || run_battle(main, &config, robots));
    let mut a = TcpStream::connect(&address).unwrap();
    write_message(&mut a, &robot_hello("a", None)).unwrap();
    let a = play(a, -1, Duration::ZERO, 0);
    thread::sleep(Duration::from_millis(500));
    let mut b = TcpStream::connect(&address).unwrap();
    write_message(&mut b, &robot_hello("b", None)).unwrap();
    let b = play(b, -1, Duration::ZERO, 0);
    let outcome = server.join().unwrap().unwrap();
    assert!(b.join().unwrap().0 > 5, "b took part");
    a.join().unwrap();
    assert_eq!(outcome.results.len(), 2);
}

#[test]
fn a_debug_battle_says_so_to_observers() {
    let config = ServerConfig {
        rules: rules(),
        observer_token: "obs".into(),
        ..Default::default()
    };
    let main = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = main.local_addr().unwrap().to_string();
    let robots = vec![entry("b.B", "b", true)];
    let server = thread::spawn(move || run_battle(main, &config, robots));
    let mut observer = TcpStream::connect(&address).unwrap();
    write_message(&mut observer, &hello(v1::Role::Observer, "obs", None)).unwrap();
    let mut b = TcpStream::connect(&address).unwrap();
    write_message(&mut b, &robot_hello("b", None)).unwrap();
    let b = play(b, -1, Duration::ZERO, 0);
    let mut debug = None;
    while let Some(msg) = receive(&mut observer) {
        if let server_message::Msg::BattleInfo(info) = msg {
            debug = Some(info.debug);
        }
    }
    server.join().unwrap().unwrap();
    b.join().unwrap();
    assert_eq!(debug, Some(true));
}

#[test]
fn the_ide_token_is_made_once() {
    let dir = std::env::temp_dir().join(format!("robocodes-ide-token-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let first = ide::token(&dir).unwrap();
    assert_eq!(first.len(), 32);
    assert_eq!(first, ide::token(&dir).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(ide::TOKEN_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "only the user reads the token");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_debug_battle_is_recorded_as_one() {
    let path = std::env::temp_dir().join(format!("robocodes-ide-{}.rec", std::process::id()));
    let config = ServerConfig {
        rules: rules(),
        record: Some(path.clone()),
        ..Default::default()
    };
    let main = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = main.local_addr().unwrap().to_string();
    let robots = vec![entry("b.B", "b", true)];
    let server = thread::spawn(move || run_battle(main, &config, robots));
    let mut b = TcpStream::connect(&address).unwrap();
    write_message(&mut b, &robot_hello("b", None)).unwrap();
    let b = play(b, -1, Duration::ZERO, 0);
    server.join().unwrap().unwrap();
    b.join().unwrap();
    let recording = robocode_s_server::recording::Recording::read(&path).unwrap();
    assert!(recording.header.debug);
    let _ = std::fs::remove_file(&path);
}
