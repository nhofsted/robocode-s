// SPDX-License-Identifier: EPL-2.0

//! Battle server: robot and observer connections, the lockstep turn loop, turn
//! deadlines, skipped turns and deterministic mode (design §5, §7; engine-spec §11).
//!
//! Threads and blocking TCP: every connection has a reader thread that forwards its
//! messages to the battle loop, which owns the engine and does all writing.

pub mod convert;
pub mod driver;
pub mod recording;

use driver::RecordingDriver;
use recording::Recorder;
use robocode_s_engine::{Battle, BattleRules, Event};
use robocode_s_protocol::v1::{self, client_message, recording_entry::Entry, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// More consecutive skipped turns disable a robot (240 for robots that did file I/O).
const MAX_SKIPPED_TURNS: i64 = 30;
const MAX_SKIPPED_TURNS_WITH_IO: i64 = 240;

/// How a battle is run.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub rules: BattleRules,
    /// Seed of the engine's randomness (classic's `RANDOMSEED`).
    pub seed: i64,
    /// Deterministic mode (ADR 0005): always wait for every robot, never skip turns.
    pub deterministic: bool,
    /// Classic mode: how long robots get per turn before they skip it.
    pub turn_timeout: Duration,
    /// Classic mode: time for a round's start and first turn (classic allows 10×).
    pub start_timeout: Duration,
    /// Deterministic mode: a robot that takes longer than this for one turn is disabled.
    pub hard_limit: Duration,
    /// How long robots get to connect before the battle starts without them.
    pub connect_timeout: Duration,
    /// Record the battle to this file (design §11, [`recording`]).
    pub record: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            rules: BattleRules::default(),
            seed: 0,
            deterministic: false,
            turn_timeout: Duration::from_millis(50),
            start_timeout: Duration::from_secs(5),
            hard_limit: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(30),
            record: None,
        }
    }
}

/// A robot expected in the battle.
#[derive(Debug, Clone)]
pub struct RobotEntry {
    pub statics: v1::RobotStatics,
    /// The one-time token the robot presents in its `Hello`.
    pub token: String,
}

/// The end of a battle.
#[derive(Debug, Clone, PartialEq)]
pub struct BattleOutcome {
    /// Final results per robot, by robot index.
    pub results: Vec<v1::Results>,
    pub rounds: i32,
}

/// Something the battle loop must handle.
enum Incoming {
    RobotConnected {
        index: usize,
        stream: TcpStream,
    },
    ObserverConnected {
        stream: TcpStream,
    },
    Message {
        robot: usize,
        message: Box<client_message::Msg>,
    },
    Control(v1::ObserverControl),
    RobotDisconnected {
        robot: usize,
    },
}

/// Per robot, what the battle loop knows about its connection.
#[derive(Default)]
struct Link {
    stream: Option<TcpStream>,
    /// The robot receives results this round: from `RoundStart` until the `Tick` in which
    /// it stops (death, halt or the end of the round).
    active: bool,
    /// After a halting `Tick`, a robot that is halted or just died sends one final
    /// `Execute`; its commands still take effect, as in classic (`executeImpl` stores them
    /// before throwing).
    final_execute: bool,
    /// The turn of the `Tick` (0: `RoundStart`) we await an `Execute` for.
    awaiting: Option<i64>,
    output: String,
    io_robot: bool,
    events: Vec<Event>,
    bullet_updates: Vec<v1::BulletStatus>,
    team_messages: Vec<v1::TeamMessage>,
}

/// Runs a battle with robots (and any observers) connecting to `listener`.
pub fn run_battle(
    listener: TcpListener,
    config: &ServerConfig,
    robots: Vec<RobotEntry>,
) -> io::Result<BattleOutcome> {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let tokens: Vec<String> = robots.iter().map(|r| r.token.clone()).collect();
    let acceptor = {
        let (tx, stop) = (tx.clone(), stop.clone());
        listener.set_nonblocking(true)?;
        thread::spawn(move || accept_loop(&listener, &tokens, &tx, &stop))
    };
    drop(tx);
    let result = BattleLoop::new(config, robots, rx).and_then(BattleLoop::run);
    stop.store(true, Ordering::SeqCst);
    let _ = acceptor.join();
    result
}

fn accept_loop(
    listener: &TcpListener,
    tokens: &[String],
    tx: &Sender<Incoming>,
    stop: &AtomicBool,
) {
    let mut taken = vec![false; tokens.len()];
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handshake(stream, tokens, &mut taken, tx);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Reads the `Hello` and hands the connection to the battle loop.
fn handshake(
    stream: TcpStream,
    tokens: &[String],
    taken: &mut [bool],
    tx: &Sender<Incoming>,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = stream.try_clone()?;
    let hello = match read_message::<v1::ClientMessage>(&mut reader)? {
        Some(v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(h)),
        }) => h,
        _ => return refuse(stream, "expected Hello"),
    };
    if hello.version.map(|v| v.major) != Some(PROTOCOL_VERSION.major) {
        return refuse(stream, "unsupported protocol version");
    }
    stream.set_read_timeout(None)?;
    match v1::Role::try_from(hello.role) {
        Ok(v1::Role::Robot) => {
            let Some(index) = tokens.iter().position(|t| *t == hello.token) else {
                return refuse(stream, "unknown robot token");
            };
            if taken[index] {
                return refuse(stream, "robot token already used");
            }
            taken[index] = true;
            spawn_robot_reader(index, reader, tx.clone());
            let _ = tx.send(Incoming::RobotConnected { index, stream });
        }
        Ok(v1::Role::Observer) => {
            spawn_observer_reader(reader, tx.clone());
            let _ = tx.send(Incoming::ObserverConnected { stream });
        }
        _ => return refuse(stream, "unsupported role"),
    }
    Ok(())
}

fn refuse(mut stream: TcpStream, message: &str) -> io::Result<()> {
    write_message(
        &mut stream,
        &server(server_message::Msg::Error(v1::Error {
            message: message.into(),
        })),
    )
}

fn spawn_robot_reader(robot: usize, mut reader: TcpStream, tx: Sender<Incoming>) {
    thread::spawn(move || {
        loop {
            match read_message::<v1::ClientMessage>(&mut reader) {
                Ok(Some(v1::ClientMessage { msg: Some(message) })) => {
                    let message = Box::new(message);
                    if tx.send(Incoming::Message { robot, message }).is_err() {
                        return;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = tx.send(Incoming::RobotDisconnected { robot });
                    return;
                }
            }
        }
    });
}

fn spawn_observer_reader(mut reader: TcpStream, tx: Sender<Incoming>) {
    thread::spawn(move || {
        while let Ok(Some(message)) = read_message::<v1::ClientMessage>(&mut reader) {
            if let Some(client_message::Msg::ObserverControl(c)) = message.msg {
                if tx.send(Incoming::Control(c)).is_err() {
                    return;
                }
            }
        }
    });
}

fn server(msg: server_message::Msg) -> v1::ServerMessage {
    v1::ServerMessage { msg: Some(msg) }
}

struct BattleLoop<'a> {
    config: &'a ServerConfig,
    statics: Vec<v1::RobotStatics>,
    /// The engine; every input goes through it and into the recording.
    engine: RecordingDriver,
    links: Vec<Link>,
    observers: Vec<TcpStream>,
    rx: Receiver<Incoming>,
    paused: bool,
    steps: u32,
    turns_per_second: u32,
}

impl<'a> BattleLoop<'a> {
    fn new(
        config: &'a ServerConfig,
        robots: Vec<RobotEntry>,
        rx: Receiver<Incoming>,
    ) -> io::Result<Self> {
        let mut statics: Vec<v1::RobotStatics> = robots.into_iter().map(|r| r.statics).collect();
        let rules = convert::rules(&config.rules, config.deterministic);
        for (i, s) in statics.iter_mut().enumerate() {
            s.robot_index = i as i32;
            s.rules = Some(rules);
        }
        let setups = statics.iter().map(convert::robot_setup).collect();
        let links = statics.iter().map(|_| Link::default()).collect();
        let recorder = match &config.record {
            Some(path) => Some(Recorder::create(
                path,
                v1::RecordingHeader {
                    format: recording::FORMAT,
                    engine: env!("CARGO_PKG_VERSION").to_string(),
                    seed: config.seed,
                    rules: Some(rules),
                    robots: statics.clone(),
                },
            )?),
            None => None,
        };
        let battle = Battle::new(config.rules.clone(), setups, config.seed);
        Ok(Self {
            config,
            engine: RecordingDriver::new(battle, recorder),
            statics,
            links,
            observers: Vec::new(),
            rx,
            paused: false,
            steps: 0,
            turns_per_second: 0,
        })
    }

    fn run(mut self) -> io::Result<BattleOutcome> {
        self.await_robots();
        for _ in 0..self.config.rules.num_rounds {
            self.run_round();
            self.engine.input(Entry::FinishRound(true));
        }
        for i in 0..self.links.len() {
            self.send(
                i,
                server_message::Msg::BattleOver(v1::BattleOver { aborted: false }),
            );
        }
        let results = recording::round_result(self.battle()).results;
        // Reader threads hold clones of the sockets; shutting them down closes the
        // connections and ends those threads.
        for stream in self
            .links
            .iter()
            .filter_map(|l| l.stream.as_ref())
            .chain(&self.observers)
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.engine.finish()?;
        Ok(BattleOutcome {
            results,
            rounds: self.config.rules.num_rounds,
        })
    }

    /// Waits until every robot connected, or the connect timeout passed.
    fn await_robots(&mut self) {
        let deadline = Instant::now() + self.config.connect_timeout;
        while self.links.iter().any(|l| l.stream.is_none()) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            match self.rx.recv_timeout(left) {
                Ok(incoming) => self.handle(incoming),
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn run_round(&mut self) {
        self.engine.input(Entry::StartRound(true));
        let round = self.battle().round;
        for i in 0..self.links.len() {
            let link = &mut self.links[i];
            link.events.clear();
            link.bullet_updates.clear();
            link.team_messages.clear();
            link.awaiting = None;
            link.active = false;
            link.final_execute = false;
            if link.stream.is_none() {
                // Could not be started: classic disables it (CANNOT_START).
                self.engine.input(Entry::Disable(i as i32));
                continue;
            }
            let driver = &self.engine.driver;
            let commands =
                convert::commands_copy(&driver.battle.robots[i].commands, &driver.passthrough[i]);
            let status = convert::status(&driver.battle.status(i));
            self.links[i].awaiting = Some(0);
            self.links[i].active = true;
            self.send(
                i,
                server_message::Msg::RoundStart(v1::RoundStart {
                    round,
                    status: Some(status),
                    commands: Some(commands),
                }),
            );
        }
        // As classic (`initializeRound` waits in `startRound` until every robot sleeps,
        // then publishes): the round start snapshot includes what robots did before their
        // first `Execute`, such as a crash's drained energy. Their console output comes
        // with turn 1 (classic's round start snapshot doesn't read out text).
        self.collect_executes();
        self.snapshot_with(false);
        loop {
            self.engine.input(Entry::Step(true));
            self.deliver();
            // As classic (`runTurn` ends with `wakeupRobots`, then `finalizeTurn` publishes):
            // a turn's snapshot includes what robots did while handling it, such as a
            // crash's drained energy and their console output.
            self.collect_executes();
            self.snapshot();
            self.pace();
            if self.battle().is_round_over() {
                break;
            }
        }
        for i in 0..self.links.len() {
            self.send(i, server_message::Msg::RoundOver(v1::RoundOver { round }));
        }
        let result = recording::round_result(self.battle());
        self.broadcast(server_message::Msg::RoundResult(result));
    }

    fn battle(&self) -> &Battle {
        &self.engine.driver.battle
    }

    /// Whether we still expect an `Execute` from this robot before the next turn.
    fn expecting(&self, i: usize) -> bool {
        let link = &self.links[i];
        link.awaiting.is_some()
            && link.stream.is_some()
            && (self.battle().robots[i].is_running() || link.final_execute)
    }

    /// Waits for the `Execute`s of this turn (engine-spec §11.3).
    fn collect_executes(&mut self) {
        let timeout = if self.config.deterministic {
            self.config.hard_limit
        } else if self.battle().time == 0 {
            self.config.start_timeout
        } else {
            self.config.turn_timeout
        };
        let deadline = Instant::now() + timeout;
        while (0..self.links.len()).any(|i| self.expecting(i)) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match self.rx.recv_timeout(left) {
                Ok(incoming) => self.handle(incoming),
                Err(_) => break,
            }
        }
        if self.config.deterministic {
            // Too slow for the hard limit: disabled.
            for i in 0..self.links.len() {
                if self.expecting(i) {
                    self.engine.input(Entry::Disable(i as i32));
                    self.links[i].awaiting = None;
                }
            }
        }
        // In classic mode, robots still busy skip this turn: their commands stay in effect.
    }

    fn handle(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::RobotConnected { index, stream } => {
                self.links[index].stream = Some(stream);
                let statics = self.statics[index].clone();
                self.send(
                    index,
                    server_message::Msg::HelloAck(v1::HelloAck {
                        version: Some(PROTOCOL_VERSION),
                        features: Vec::new(),
                        robot: Some(statics),
                    }),
                );
            }
            Incoming::ObserverConnected { mut stream } => {
                let info = v1::BattleInfo {
                    rules: Some(convert::rules(
                        &self.config.rules,
                        self.config.deterministic,
                    )),
                    robots: self.statics.clone(),
                };
                let ack = server(server_message::Msg::HelloAck(v1::HelloAck {
                    version: Some(PROTOCOL_VERSION),
                    ..Default::default()
                }));
                if write_message(&mut stream, &ack).is_ok()
                    && write_message(&mut stream, &server(server_message::Msg::BattleInfo(info)))
                        .is_ok()
                {
                    self.observers.push(stream);
                }
            }
            Incoming::Message { robot, message } => {
                if let client_message::Msg::Execute(execute) = *message {
                    self.on_execute(robot, execute);
                }
            }
            Incoming::Control(control) => self.on_control(control),
            Incoming::RobotDisconnected { robot } => {
                self.links[robot].stream = None;
                self.links[robot].awaiting = None;
            }
        }
    }

    fn on_execute(&mut self, i: usize, execute: v1::Execute) {
        let link = &mut self.links[i];
        if link.awaiting != Some(execute.turn) {
            return; // not the reply we wait for
        }
        link.awaiting = None;
        link.final_execute = false;
        link.output.push_str(&execute.output);
        let robot = i as i32;
        if let Some(c) = execute.commands {
            link.io_robot |= c.io_robot;
            self.engine.input(Entry::Commands(v1::RecordedCommands {
                robot,
                commands: Some(c),
            }));
        }
        if execute.drain_energy {
            self.engine.input(Entry::DrainEnergy(robot));
        }
        if execute.disable {
            self.engine.input(Entry::Disable(robot));
        }
        if execute.stopped {
            self.links[i].active = false;
            self.engine.input(Entry::Stop(robot));
        }
        // `checkSkippedTurn`: turns that ran since the Tick this robot answers.
        let time = self.battle().time;
        let alive = self.battle().robots[i].is_alive();
        let skipped = time - execute.turn;
        if skipped >= 1 {
            let link = &mut self.links[i];
            link.events.retain(convert::is_critical);
            if alive {
                for turn in execute.turn + 1..=time {
                    link.events.push(Event::SkippedTurn { turn });
                }
            }
            let max = if link.io_robot {
                MAX_SKIPPED_TURNS_WITH_IO
            } else {
                MAX_SKIPPED_TURNS
            };
            if skipped > max {
                self.engine.input(Entry::DiscardCommands(robot));
                self.engine.input(Entry::Disable(robot));
            }
        }
    }

    fn on_control(&mut self, control: v1::ObserverControl) {
        match control.control {
            Some(v1::observer_control::Control::Pause(pause)) => self.paused = pause,
            Some(v1::observer_control::Control::Step(true)) => self.steps += 1,
            Some(v1::observer_control::Control::TurnsPerSecond(tps)) => self.turns_per_second = tps,
            _ => {}
        }
    }

    /// Sends each robot that answered its last `Tick` the results of this turn. A robot
    /// that is still busy (skipping turns) gets them, accumulated, after its `Execute`.
    fn deliver(&mut self) {
        let turn = self.battle().time;
        let round_over = self.battle().is_round_over();
        let deterministic = self.config.deterministic;
        let budget = self.config.turn_timeout.as_nanos() as u64;
        for i in 0..self.links.len() {
            let driver = &mut self.engine.driver;
            let battle = &mut driver.battle;
            let name = battle.robots[i].setup.name.clone();
            let events = battle.take_events(i);
            let bullets = battle.take_bullet_updates(i);
            let messages = battle.take_team_messages(i);
            let robot = &battle.robots[i];
            let running = robot.is_running() && !round_over;
            let alive = robot.is_alive();
            // A robot whose thread still runs into this turn's stop: halted (alive), or just
            // died (not halted; a disabled robot is both dead and halted, and has stopped).
            let final_execute = !round_over && (alive || !robot.is_halted());
            let should_wait = battle.aborted || (battle.is_last_round() && robot.is_winner());
            let status = convert::status(&battle.status(i));
            let commands = convert::commands_copy(&robot.commands, &driver.passthrough[i]);
            let link = &mut self.links[i];
            if !link.active {
                continue;
            }
            link.events.extend(events);
            link.bullet_updates
                .extend(bullets.iter().map(convert::bullet_status));
            link.team_messages
                .extend(messages.iter().map(convert::team_message));
            if link.stream.is_none() || link.awaiting.is_some() {
                continue;
            }
            let tick = v1::Tick {
                turn,
                status: Some(status),
                commands: Some(commands),
                events: link
                    .events
                    .drain(..)
                    .map(|e| convert::event(&e, &name))
                    .collect(),
                team_messages: std::mem::take(&mut link.team_messages),
                bullet_updates: std::mem::take(&mut link.bullet_updates),
                halt: !running,
                should_wait,
                paint_enabled: false,
                budget_nanos: if deterministic { 0 } else { budget },
            };
            if running {
                link.awaiting = Some(turn);
            } else {
                link.active = false;
                if final_execute {
                    // Halted or just died: one final Execute still counts. Classic stores
                    // it before throwing, and loads a dead robot's commands too, so a robot
                    // that regained energy in the turn it died fires once more (a ghost
                    // shot, engine-spec §15.3).
                    link.awaiting = Some(turn);
                    link.final_execute = true;
                }
            }
            self.send(i, server_message::Msg::Tick(tick));
        }
    }

    fn snapshot(&mut self) {
        self.snapshot_with(true);
    }

    /// Broadcasts the current turn; `with_output` takes the robots' console output so far.
    fn snapshot_with(&mut self, with_output: bool) {
        if self.observers.is_empty() && !self.engine.is_recording() {
            return;
        }
        let links = &mut self.links;
        let snapshot = self.engine.driver.snapshot(&mut |i| {
            if with_output {
                std::mem::take(&mut links[i].output)
            } else {
                String::new()
            }
        });
        if self.engine.is_recording() {
            let marker = v1::RecordedSnapshot {
                output: snapshot.robots.iter().map(|r| r.output.clone()).collect(),
                paint: Vec::new(),
                check: recording::check(&snapshot),
            };
            self.engine.record(&Entry::Snapshot(marker));
        }
        self.broadcast(server_message::Msg::TurnSnapshot(snapshot));
    }

    /// Observer pacing: pause, single steps and a turn rate.
    fn pace(&mut self) {
        while self.paused && self.steps == 0 && !self.observers.is_empty() {
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(incoming) => self.handle(incoming),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.paused && self.steps > 0 {
            self.steps -= 1;
        }
        if self.turns_per_second > 0 {
            thread::sleep(Duration::from_secs_f64(
                1.0 / f64::from(self.turns_per_second),
            ));
        }
    }

    fn send(&mut self, i: usize, msg: server_message::Msg) {
        if let Some(stream) = self.links[i].stream.as_mut() {
            if write_message(stream, &server(msg)).is_err() {
                self.links[i].stream = None;
                self.links[i].awaiting = None;
            }
        }
    }

    fn broadcast(&mut self, msg: server_message::Msg) {
        let message = server(msg);
        self.observers
            .retain_mut(|o| write_message(o, &message).is_ok());
    }
}
