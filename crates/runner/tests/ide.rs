// SPDX-License-Identifier: EPL-2.0

//! A battle of robots from an IDE (#17), played by in-process fake robots: they connect to
//! the IDE listener and describe themselves; the battle takes them from there, names them
//! uniquely and waits for them. No Java needed.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_runner::Sandbox;
use robocode_s_runner::ide::marker;
use robocode_s_runner::local::{Environment, prepare_ide, run_local_battle};
use robocode_s_server::ServerConfig;
use robocode_s_server::ide::IdeListener;
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

/// A robot that describes itself as `name`, then answers every turn; it reports the name
/// the battle gave it and the turns it saw.
fn fake_robot(
    port: u16,
    name: &str,
    delay_at: Option<(i64, Duration)>,
) -> thread::JoinHandle<(String, i64)> {
    let name = name.to_string();
    thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let hello = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(v1::Hello {
                version: Some(PROTOCOL_VERSION),
                role: v1::Role::Robot as i32,
                token: "secret".into(),
                robot_description: Some(v1::RobotDescription {
                    name: name.clone(),
                    full_class_name: name,
                    r#type: Some(v1::RobotType {
                        advanced: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            })),
        };
        write_message(&mut stream, &hello).unwrap();
        let execute = |turn| v1::ClientMessage {
            msg: Some(client_message::Msg::Execute(v1::Execute {
                turn,
                commands: Some(v1::RobotCommands::default()),
                ..Default::default()
            })),
        };
        let (mut called, mut last) = (String::new(), 0);
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut stream) {
            match message.msg {
                Some(server_message::Msg::HelloAck(ack)) => {
                    called = ack.robot.unwrap_or_default().name;
                }
                Some(server_message::Msg::RoundStart(_)) => {
                    write_message(&mut stream, &execute(0)).unwrap();
                }
                Some(server_message::Msg::Tick(t)) => {
                    last = t.turn;
                    if let Some((turn, delay)) = delay_at
                        && t.turn == turn
                    {
                        thread::sleep(delay);
                    }
                    let _ = write_message(&mut stream, &execute(t.turn));
                }
                _ => {}
            }
        }
        (called, last)
    })
}

#[test]
fn a_battle_of_robots_from_an_ide_names_them_and_waits_for_them() {
    let dir = std::env::temp_dir().join(format!("robocodes-ide-battle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ide = IdeListener::bind("127.0.0.1:0", "secret".into()).unwrap();
    let port = ide.local_addr().port();
    // Both are the same class and the second is slow: ten times the turn deadline.
    let first = fake_robot(port, "my.Robot*", None);
    ide.wait_for(1, &AtomicBool::new(false)).unwrap();
    let second = fake_robot(port, "my.Robot*", Some((3, Duration::from_millis(500))));
    let here = ide.wait_for(2, &AtomicBool::new(false)).unwrap();
    let env = Environment {
        java: Some("java".into()),
        host_lib: dir.clone(),
        agent_jar: dir.join("agent.jar"),
        data: dir.join("data"),
        jars: Vec::new(),
        sandbox: Sandbox::None,
        fresh_data: true,
    };
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 1,
            inactivity_time: 5,
            ..Default::default()
        },
        turn_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let robots: Vec<String> = here.iter().map(|r| marker(r.id)).collect();
    let (host, battle) = prepare_ide(&env, &robots, Some(&ide), config, 128, None).unwrap();
    let names: Vec<&str> = battle
        .robots
        .iter()
        .map(|r| r.statics.name.as_str())
        .collect();
    assert_eq!(names, ["my.Robot* (1)", "my.Robot* (2)"]);
    let mut debug = false;
    let local = run_local_battle(&host, &battle, None, |msg| {
        if let server_message::Msg::BattleInfo(info) = msg {
            debug = info.debug;
        }
    })
    .unwrap();
    assert!(debug, "observers are told it is a debug battle");
    assert_eq!(local.outcome.results.len(), 2);
    assert_eq!(local.stderr, ["", ""]);
    let (called, turns) = first.join().unwrap();
    assert_eq!(called, "my.Robot* (1)");
    assert!(turns > 5, "{turns} turns");
    let (called, turns) = second.join().unwrap();
    assert_eq!(called, "my.Robot* (2)");
    assert!(turns > 5, "waited for the slow one: {turns} turns");
    // One session, one battle: the robots are not offered again.
    assert!(ide.robots().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_robot_that_left_before_the_start_says_so() {
    let dir = std::env::temp_dir().join(format!("robocodes-ide-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ide = IdeListener::bind("127.0.0.1:0", "secret".into()).unwrap();
    let robots = vec![marker(99)];
    let env = Environment {
        java: Some("java".into()),
        host_lib: dir.clone(),
        agent_jar: dir.join("agent.jar"),
        data: dir.join("data"),
        jars: Vec::new(),
        sandbox: Sandbox::None,
        fresh_data: true,
    };
    let config = ServerConfig::default();
    let err = prepare_ide(&env, &robots, Some(&ide), config, 128, None).unwrap_err();
    assert!(err.contains("isn't connected"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
