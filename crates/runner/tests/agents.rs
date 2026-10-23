// SPDX-License-Identifier: EPL-2.0

//! Runner agents (ADR 0020) over TLS on loopback: the agent's and the battle's sides
//! together, with robots that are threads of this test (no Java), then (ignored: needs
//! Java and the built robots) with real robot JVMs, whose battle must be the battle run
//! locally.

use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_runner::agent::{self, AgentConfig, Ended, Launcher, Robot};
use robocode_s_runner::local::{LocalBattle, LocalRobot};
use robocode_s_runner::remote::run_remote_battle;
use robocode_s_runner::{RobotLaunch, RobotStdin, RobotStdout};
use robocode_s_server::agents::AgentListener;
use robocode_s_server::tls::{Fingerprint, Identity};
use robocode_s_server::{RobotEntry, RobotPipe, ServerConfig, net, run_battle_piped};
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// One end of an in-memory pipe.
struct PipeReader {
    rx: Receiver<Vec<u8>>,
    buffer: Vec<u8>,
}

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.buffer.is_empty() {
            match self.rx.recv() {
                Ok(bytes) => self.buffer = bytes,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.buffer.len());
        buf[..n].copy_from_slice(&self.buffer[..n]);
        self.buffer.drain(..n);
        Ok(n)
    }
}

struct PipeWriter(Sender<Vec<u8>>);

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .send(buf.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn pipe() -> (PipeWriter, PipeReader) {
    let (tx, rx) = mpsc::channel();
    (
        PipeWriter(tx),
        PipeReader {
            rx,
            buffer: Vec::new(),
        },
    )
}

/// A robot that says `Hello` and answers every turn with no commands, on a thread: the
/// pipes the runner would get from its process.
fn idle_robot(last_turn: Arc<AtomicUsize>) -> (RobotStdout, RobotStdin) {
    let (to_robot, mut robot_in) = pipe();
    let (mut robot_out, from_robot) = pipe();
    thread::spawn(move || {
        let hello = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(v1::Hello {
                version: Some(PROTOCOL_VERSION),
                role: v1::Role::Robot as i32,
                ..Default::default()
            })),
        };
        if write_message(&mut robot_out, &hello).is_err() {
            return;
        }
        while let Ok(Some(m)) = read_message::<v1::ServerMessage>(&mut robot_in) {
            let turn = match m.msg {
                Some(server_message::Msg::RoundStart(_)) => 0,
                Some(server_message::Msg::Tick(t)) => t.turn,
                _ => continue,
            };
            last_turn.fetch_max(turn as usize, Ordering::SeqCst);
            let execute = v1::ClientMessage {
                msg: Some(client_message::Msg::Execute(v1::Execute {
                    turn,
                    commands: Some(v1::RobotCommands::default()),
                    ..Default::default()
                })),
            };
            if write_message(&mut robot_out, &execute).is_err() {
                return;
            }
        }
    });
    (Box::new(from_robot), Box::new(to_robot))
}

/// Starts [`idle_robot`]s; what it was asked to start.
#[derive(Default)]
struct FakeLauncher {
    name: String,
    launched: Mutex<Vec<RobotLaunch>>,
    last_turn: Arc<AtomicUsize>,
}

struct FakeRobot {
    pipes: Option<(RobotStdout, RobotStdin)>,
}

impl Robot for FakeRobot {
    fn take_pipes(&mut self) -> Option<(RobotStdout, RobotStdin)> {
        self.pipes.take()
    }
    fn kill(&mut self) {}
    fn exit_code(&mut self) -> Option<i32> {
        Some(0)
    }
    fn stderr_tail(&self) -> String {
        "fake robot's error output".into()
    }
}

impl Launcher for FakeLauncher {
    fn info(&self) -> (String, u32) {
        (
            format!("fake {}", self.name),
            robocode_s_server::DETERMINISTIC_RULES,
        )
    }

    fn launch(&self, launch: &RobotLaunch) -> io::Result<Box<dyn Robot>> {
        for jar in &launch.jars {
            assert!(jar.is_file(), "{} fetched", jar.display());
        }
        self.launched.lock().unwrap().push(launch.clone());
        Ok(Box::new(FakeRobot {
            pipes: Some(idle_robot(self.last_turn.clone())),
        }))
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("robocodes-agents-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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

fn config() -> ServerConfig {
    ServerConfig {
        rules: BattleRules {
            num_rounds: 2,
            inactivity_time: 30,
            ..Default::default()
        },
        seed: 7,
        deterministic: true,
        ..Default::default()
    }
}

fn battle(dir: &Path, names: &[&str]) -> LocalBattle {
    let jar = dir.join("robots.jar");
    std::fs::write(&jar, b"PK not really a jar").unwrap();
    LocalBattle {
        config: config(),
        robots: names
            .iter()
            .map(|n| LocalRobot {
                statics: statics(n),
                lookup: n.to_string(),
                ide: None,
            })
            .collect(),
        ide: None,
        jars: vec![jar],
        dir: dir.join("battle"),
        data: None,
        memory_mb: 128,
        pool: None,
    }
}

/// A listener with TLS on loopback, as `--tls` makes it.
fn listener(dir: &Path) -> (AgentListener, Fingerprint) {
    let identity = Identity::load_or_create(&dir.join("server")).unwrap();
    let listener =
        AgentListener::bind("127.0.0.1:0", Some(identity), "agent-token".into()).unwrap();
    let pin = listener.fingerprint().unwrap();
    (listener, pin)
}

fn agent_config(listener: &AgentListener, pin: Fingerprint, dir: &Path) -> AgentConfig {
    AgentConfig {
        server: listener.local_addr().to_string(),
        token: "agent-token".into(),
        pin: Some(pin),
        slots: 0,
        dir: dir.to_path_buf(),
        fresh_data: false,
    }
}

/// What an agent said.
type Said = Arc<Mutex<Vec<String>>>;

/// Runs the agent on a thread until `stop`; what it said.
fn spawn_agent(
    config: AgentConfig,
    launcher: Arc<FakeLauncher>,
    stop: Arc<AtomicBool>,
) -> (thread::JoinHandle<Result<(), String>>, Said) {
    let said = Arc::new(Mutex::new(Vec::new()));
    let log = said.clone();
    let handle = thread::spawn(move || {
        agent::run(&config, launcher.as_ref(), &stop, &mut |s| {
            log.lock().unwrap().push(s.to_string())
        })
    });
    (handle, said)
}

#[test]
fn a_battle_on_an_agent_is_the_battle_on_pipes_and_its_jars_are_fetched_once() {
    let dir = temp("same");
    let (listener, pin) = listener(&dir);
    let launcher = Arc::new(FakeLauncher::default());
    let stop = Arc::new(AtomicBool::new(false));
    let (agent, said) = spawn_agent(
        agent_config(&listener, pin, &dir.join("agent")),
        launcher.clone(),
        stop.clone(),
    );
    let battle = battle(&dir, &["a.A", "b.B", "a.A (2)"]);

    let remote = run_remote_battle(
        &battle,
        &listener,
        (1, Duration::from_secs(20)),
        None,
        None,
        |_| {},
    )
    .unwrap();
    assert_eq!(remote.agents.len(), 3);
    assert!(remote.agents[0].contains("fake"), "{:?}", remote.agents);
    assert_eq!(remote.local.stderr, vec!["fake robot's error output"; 3]);
    let launched = launcher.launched.lock().unwrap().clone();
    assert_eq!(launched.len(), 3);
    assert_eq!(launched[2].robot, "a.A (2)");
    assert_eq!((launched[0].seed, launched[0].deterministic), (7, true));

    // The same robots on pipes, without an agent.
    let robots: Vec<RobotEntry> = battle
        .robots
        .iter()
        .map(|r| RobotEntry {
            statics: r.statics.clone(),
            token: "unused".into(),
            debug: false,
        })
        .collect();
    let pipes = (0..robots.len())
        .map(|robot| {
            let (reader, writer) = idle_robot(Arc::default());
            RobotPipe {
                robot,
                reader,
                writer,
            }
        })
        .collect();
    let local = run_battle_piped(
        TcpListener::bind("127.0.0.1:0").unwrap(),
        None,
        &config(),
        robots,
        pipes,
    )
    .unwrap();
    assert_eq!(remote.local.outcome, local);
    assert_eq!(local.rounds, 2);

    // The agent came back for another battle: its jar is in its cache.
    let again = run_remote_battle(
        &battle,
        &listener,
        (1, Duration::from_secs(20)),
        None,
        None,
        |_| {},
    )
    .unwrap();
    assert_eq!(again.local.outcome, local);
    stop.store(true, Ordering::SeqCst);
    agent.join().unwrap().unwrap();
    let fetched = said
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.starts_with("fetched robots.jar"))
        .count();
    assert_eq!(fetched, 1, "{:?}", said.lock().unwrap());
    listener.close();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agents_with_the_wrong_token_certificate_or_version_are_refused() {
    let dir = temp("refused");
    let (listener, pin) = listener(&dir);
    let launcher = FakeLauncher::default();
    let stop = AtomicBool::new(false);
    let mut config = agent_config(&listener, pin, &dir);

    config.token = "guess".into();
    let e = agent::session(&config, &launcher, &stop, &mut |_| {}).unwrap_err();
    assert!(
        matches!(&e, Ended::Refused(e) if e.contains("wrong agent token")),
        "{e:?}"
    );

    config.token = "agent-token".into();
    config.pin = Some(Fingerprint([1; 32]));
    let e = agent::session(&config, &launcher, &stop, &mut |_| {}).unwrap_err();
    assert!(
        matches!(&e, Ended::Refused(e) if e.contains("not the pinned")),
        "{e:?}"
    );
    // The agent stops rather than trying again.
    let e = agent::run(&config, &launcher, &stop, &mut |_| {}).unwrap_err();
    assert!(e.contains("not the pinned"), "{e}");

    // Another engine version or other deterministic rules.
    for (engine, rules, why) in [
        (
            robocode_s_engine::ENGINE_VERSION + 1,
            robocode_s_server::DETERMINISTIC_RULES,
            "engine version",
        ),
        (
            robocode_s_engine::ENGINE_VERSION,
            robocode_s_server::DETERMINISTIC_RULES + 1,
            "deterministic rules",
        ),
    ] {
        let address = listener.local_addr().to_string();
        let mut stream = net::connect(&address, Some(pin), Duration::from_secs(5)).unwrap();
        let hello = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(v1::Hello {
                version: Some(PROTOCOL_VERSION),
                role: v1::Role::Runner as i32,
                token: "agent-token".into(),
                runner: Some(v1::RunnerInfo {
                    engine_version: engine,
                    deterministic_rules: rules,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        };
        write_message(&mut stream, &hello).unwrap();
        let reply = read_message::<v1::ServerMessage>(&mut stream)
            .unwrap()
            .and_then(|m| m.msg);
        assert!(
            matches!(&reply, Some(server_message::Msg::Error(e)) if e.message.contains(why)),
            "{reply:?}"
        );
    }

    // Robots with a token the listener doesn't expect.
    let address = listener.local_addr().to_string();
    let mut stream = net::connect(&address, Some(pin), Duration::from_secs(5)).unwrap();
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Robot as i32,
            token: "agent-token".into(),
            ..Default::default()
        })),
    };
    write_message(&mut stream, &hello).unwrap();
    let reply = read_message::<v1::ServerMessage>(&mut stream)
        .unwrap()
        .and_then(|m| m.msg);
    assert!(
        matches!(&reply, Some(server_message::Msg::Error(e)) if e.message.contains("unknown robot token")),
        "{reply:?}"
    );
    // And no plain connections: the listener wants TLS.
    let mut plain = net::connect(&address, None, Duration::from_secs(5)).unwrap();
    assert!(!plain.is_tls());
    write_message(&mut plain, &hello).unwrap();
    let reply = read_message::<v1::ServerMessage>(&mut plain);
    assert!(
        !matches!(
            reply,
            Ok(Some(v1::ServerMessage {
                msg: Some(server_message::Msg::HelloAck(_) | server_message::Msg::Error(_))
            }))
        ),
        "{reply:?}"
    );
    listener.close();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_battle_goes_on_when_an_agent_leaves_and_without_its_robots() {
    let dir = temp("leaves");
    let (listener, pin) = listener(&dir);
    let fake = |name: &str| {
        Arc::new(FakeLauncher {
            name: name.into(),
            ..FakeLauncher::default()
        })
    };
    let (stays, leaves) = (fake("stays"), fake("leaves"));
    let (stop_stays, stop_leaves) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let config = |name: &str| AgentConfig {
        slots: 1,
        ..agent_config(&listener, pin, &dir.join(name))
    };
    let (a, _) = spawn_agent(config("stays"), stays.clone(), stop_stays.clone());
    let (b, _) = spawn_agent(config("leaves"), leaves.clone(), stop_leaves.clone());
    let mut battle = battle(&dir, &["a.A", "b.B"]);
    battle.config.rules.inactivity_time = 300;
    let left = stop_leaves.clone();
    // 1000 turns a second: the agent leaves within a few dozen turns.
    let (controls, paced) = mpsc::channel();
    controls
        .send(v1::ObserverControl {
            control: Some(v1::observer_control::Control::TurnsPerSecond(1000)),
        })
        .unwrap();
    let remote = run_remote_battle(
        &battle,
        &listener,
        (2, Duration::from_secs(20)),
        None,
        Some(paced),
        move |msg| {
            if let server_message::Msg::TurnSnapshot(s) = msg
                && s.round == 0
                && s.turn == 20
            {
                left.store(true, Ordering::SeqCst);
            }
        },
    )
    .unwrap();
    assert_eq!(remote.local.outcome.rounds, 2, "the battle finished");
    let gone = leaves.last_turn.load(Ordering::SeqCst);
    let stayed = stays.last_turn.load(Ordering::SeqCst);
    assert!(
        gone < 200,
        "the robot of the agent that left stopped at turn {gone}"
    );
    assert!(stayed > gone, "the other one went on, to turn {stayed}");
    let on_it = remote
        .agents
        .iter()
        .position(|a| a.contains("fake leaves"))
        .unwrap();
    let tail = &remote.local.stderr[on_it];
    assert!(tail.contains("runner agent left"), "{tail}");
    stop_stays.store(true, Ordering::SeqCst);
    a.join().unwrap().unwrap();
    b.join().unwrap().unwrap();
    listener.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The paths of a built source tree, from the repository root.
fn environment() -> robocode_s_runner::local::Environment {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = robocode_s_runner::local::Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    env
}

#[test]
#[ignore = "needs Java and the built robots"]
fn real_robots_on_an_agent_fight_the_battle_they_fight_locally() {
    use robocode_s_runner::Sandbox;
    use robocode_s_runner::agent::JvmLauncher;
    use robocode_s_runner::local::{prepare, run_local_battle};
    let env = environment();
    let robots = [
        "sample.Fire",
        "sample.Walls",
        "sample.SpinBot",
        "sample.Tracker",
    ]
    .map(String::from);
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 3,
            ..Default::default()
        },
        seed: 11,
        deterministic: true,
        ..Default::default()
    };
    let (host, battle) = prepare(&env, &robots, config, 256).unwrap();
    let local = run_local_battle(&host, &battle, None, |_| {}).unwrap();
    let dir = temp("jvm");
    let (listener, pin) = listener(&dir);
    // The agent's robots start cold, or from its warm pool.
    for (sandbox, warm) in [
        (Sandbox::None, 0),
        (Sandbox::None, 2),
        (Sandbox::Os, 0),
        (Sandbox::Os, 2),
    ] {
        if let Some(why) = sandbox.unavailable() {
            eprintln!("{sandbox} skipped: {why}");
            continue;
        }
        let agent_env = robocode_s_runner::local::Environment {
            sandbox,
            jars: Vec::new(),
            ..env.clone()
        };
        let agent_dir = dir.join(format!("agent-{sandbox}-{warm}"));
        let launcher = JvmLauncher::new(agent_env.host(true).unwrap(), &agent_dir, warm).unwrap();
        assert!(launcher.info().0.contains(&format!("sandbox {sandbox}")));
        let stop = Arc::new(AtomicBool::new(false));
        let config = agent_config(&listener, pin, &agent_dir);
        let agent = {
            let stop = stop.clone();
            thread::spawn(move || {
                agent::run(&config, &launcher, &stop, &mut |s| eprintln!("agent: {s}"))
            })
        };
        let remote = run_remote_battle(
            &battle,
            &listener,
            (1, Duration::from_secs(30)),
            None,
            None,
            |_| {},
        )
        .unwrap();
        stop.store(true, Ordering::SeqCst);
        agent.join().unwrap().unwrap();
        assert_eq!(
            remote.local.outcome, local.outcome,
            "sandbox {sandbox}, warm {warm}"
        );
    }
    let _ = std::fs::remove_dir_all(&battle.dir);
    listener.close();
    let _ = std::fs::remove_dir_all(&dir);
}
