// SPDX-License-Identifier: EPL-2.0

//! Battle server: robot and observer connections, the lockstep turn loop, turn
//! deadlines, skipped turns and deterministic mode (design §5, §7; engine-spec §11).
//!
//! Threads and blocking TCP (TLS from elsewhere, [`net`]): every connection has a reader
//! thread that forwards its messages to the battle loop, which owns the engine and does
//! all writing.

pub mod agents;
pub mod battle_file;
pub mod convert;
pub mod driver;
pub mod net;
pub mod recording;
pub mod results;
pub mod tls;
pub mod watch;

use agents::AgentListener;
use driver::RecordingDriver;
use net::Stream;
use recording::Recorder;
use robocode_s_engine::{Battle, BattleRules, Event};
use robocode_s_protocol::v1::{self, client_message, recording_entry::Entry, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// At most this many observers at once on an [`Observers`] listener.
const MAX_REMOTE_OBSERVERS: usize = 16;

/// At most this many observers' handshakes (up to the token check) at once.
const MAX_HANDSHAKES: usize = 32;

/// An observer from elsewhere that takes no data for this long is dropped (a stalled
/// connection would otherwise hold the battle up: the battle loop does all writing).
const REMOTE_OBSERVER_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// More consecutive skipped turns disable a robot (240 for robots that did file I/O).
const MAX_SKIPPED_TURNS: i64 = 30;
const MAX_SKIPPED_TURNS_WITH_IO: i64 = 240;

/// Deterministic mode: the counted work (as virtual time, ADR 0014) a robot may do in a
/// turn before it skips turns. How long classic waits for a robot on the machine the
/// virtual clock was calibrated on (ADR 0018): 3 whole-millisecond waits for its CPU
/// constant of 2.52 ms, 3.28 ms on Linux, less classic's own robot-side work per turn
/// (64 µs), which it times too but isn't counted. Part of the deterministic rules, like the
/// clock's constants (`RobotRandomness.TURN_BUDGET_NANOS` on the Java side).
pub const DETERMINISTIC_BUDGET_NANOS: u64 = 3_200_000;

/// Budgets a robot may use in turn 1: classic waits 21 milliseconds then instead of 3.
pub const FIRST_TURN_BUDGETS: u64 = 7;

/// Version of the deterministic rules (`RobotRandomness.rulesVersion()` on the Java side):
/// deterministic battles need a robot host that follows the same ones.
pub const DETERMINISTIC_RULES: u32 = 1;

/// How many turns late a robot's `Execute` for `turn` counts in deterministic mode, after
/// `work` nanoseconds of counted work. As classic, which waits a budget for each robot each
/// turn (seven on turn 1, `Battle.wakeupRobots`) and otherwise runs the turn without it: the
/// robot answers after the turns whose wait ran out. A round's start (`turn` 0) is free.
pub fn late_turns(work: u64, turn: i64, budget: u64) -> i64 {
    let allowance = if turn == 1 {
        FIRST_TURN_BUDGETS * budget
    } else {
        budget
    };
    if turn < 1 || work < allowance {
        0
    } else {
        1 + ((work - allowance) / budget) as i64
    }
}

/// How a battle is run.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub rules: BattleRules,
    /// Seed of the engine's randomness (classic's `RANDOMSEED`).
    pub seed: i64,
    /// Deterministic mode (ADR 0005): always wait for every robot; robots skip turns by
    /// their counted work instead of time ([`late_turns`]).
    pub deterministic: bool,
    /// Classic mode: how long robots get per turn before they skip it.
    pub turn_timeout: Duration,
    /// Classic mode: time for a round's start and first turn (classic allows 10×).
    pub start_timeout: Duration,
    /// Deterministic mode: a robot that takes longer than this for one turn is disabled.
    pub hard_limit: Duration,
    /// How long robots get to connect before the battle starts without them.
    pub connect_timeout: Duration,
    /// Record the battle to this file (design §11, [`recording`]); a battle stopped
    /// before its first round leaves none.
    pub record: Option<PathBuf>,
    /// The token observers present on the robots' listener (the embedded observer of a
    /// local battle, [`random_token`]); empty: none (tests). Observers from elsewhere
    /// use [`Observers`] and its token instead.
    pub observer_token: String,
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
            observer_token: String::new(),
        }
    }
}

/// A fresh unpredictable token (128 bits from the operating system's randomness, as hex)
/// for a robot or an observer of one battle.
pub fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A robot expected in the battle.
#[derive(Debug, Clone)]
pub struct RobotEntry {
    pub statics: v1::RobotStatics,
    /// The one-time token the robot presents in its `Hello` ([`random_token`]: any local
    /// process can reach the robots' listener).
    pub token: String,
}

/// Observers from elsewhere (design §10: a remote GUI attached as an observer): a
/// listener of their own beside the robots', which takes only observers. Robots' tokens
/// never reach it, so the robots' listener can stay on loopback.
#[derive(Debug)]
pub struct Observers {
    pub listener: TcpListener,
    /// The credential observers present in their `Hello`; empty: none (for a loopback
    /// listener only, as the CLI ensures).
    pub token: String,
    /// TLS with this certificate; `None`: plain, which only a loopback listener may be
    /// ([`net::check_listener`]).
    pub tls: Option<tls::Identity>,
    /// The battle starts only once the first observer is here (so that it sees it all).
    pub wait: bool,
}

/// The end of a battle.
#[derive(Debug, Clone, PartialEq)]
pub struct BattleOutcome {
    /// Final results per robot, by robot index.
    pub results: Vec<v1::Results>,
    /// Rounds fought: all of them, or up to the one an observer stopped.
    pub rounds: i32,
    /// An observer stopped the battle (`ObserverControl.abort`).
    pub aborted: bool,
}

/// Something the battle loop must handle.
enum Incoming {
    RobotConnected {
        index: usize,
        stream: Conn,
    },
    ObserverConnected {
        stream: Stream,
        /// It came to the [`Observers`] listener.
        remote: bool,
    },
    /// An observer's connection ended.
    ObserverLeft {
        remote: bool,
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

/// A robot's connection from the battle loop's side: a socket, or the pipes of a process
/// the runner started (ADR 0019). The battle loop writes; a reader thread has the other half.
enum Conn {
    Tcp(Stream),
    /// `None` once shut down: dropping the writer closes the robot's stdin.
    Pipe(Option<Box<dyn Write + Send>>),
}

impl Conn {
    /// Ends the connection, and with it the reader thread.
    fn shutdown(&mut self) {
        match self {
            Conn::Tcp(s) => {
                let _ = s.shutdown();
            }
            Conn::Pipe(w) => *w = None,
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.write(buf),
            Conn::Pipe(Some(w)) => w.write(buf),
            Conn::Pipe(None) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Tcp(s) => s.flush(),
            Conn::Pipe(Some(w)) => w.flush(),
            Conn::Pipe(None) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }
}

/// A robot the runner started with its stdin and stdout as the connection (design §5.1):
/// no listener, no token; the pipe pair is the robot's identity. It still says `Hello`.
pub struct RobotPipe {
    /// Index of the robot in the battle's [`RobotEntry`] list.
    pub robot: usize,
    /// What the robot writes (its stdout).
    pub reader: Box<dyn Read + Send>,
    /// What the robot reads (its stdin); dropped when the battle ends.
    pub writer: Box<dyn Write + Send>,
}

/// Per robot, what the battle loop knows about its connection.
#[derive(Default)]
struct Link {
    stream: Option<Conn>,
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
    /// Debug paint of the robot's last `Execute`, for the next snapshot.
    paint: Vec<v1::PaintOp>,
    /// An observer turned this robot's debug painting on (`PaintToggle`).
    paint_enabled: bool,
    /// CPU time of the `Execute`s since the last snapshot, for the next one.
    cpu_nanos: u64,
    /// Debug properties the robot set since the last snapshot, in order, for the next one.
    debug_properties: Vec<v1::DebugProperty>,
    io_robot: bool,
    events: Vec<Event>,
    /// Observers' mouse and keyboard events for the next `Tick` (interactive robots).
    input: Vec<v1::Event>,
    bullet_updates: Vec<v1::BulletStatus>,
    team_messages: Vec<v1::TeamMessage>,
    /// Deterministic mode: an `Execute` that counts as late, and the turn it counts as
    /// arriving in ([`late_turns`]).
    late_execute: Option<(i64, v1::Execute)>,
    /// The turn of the `Tick` a robot answered late; it skipped the turns after it.
    answered_late: Option<i64>,
}

/// Runs a battle with robots (and any observers) connecting to `listener`.
pub fn run_battle(
    listener: TcpListener,
    config: &ServerConfig,
    robots: Vec<RobotEntry>,
) -> io::Result<BattleOutcome> {
    run_battle_with(listener, None, config, robots)
}

/// [`run_battle`], with observers from elsewhere on a listener of their own.
pub fn run_battle_with(
    listener: TcpListener,
    observers: Option<Observers>,
    config: &ServerConfig,
    robots: Vec<RobotEntry>,
) -> io::Result<BattleOutcome> {
    run_battle_piped(listener, observers, config, robots, Vec::new())
}

/// [`run_battle_with`], where the robots in `pipes` are connected through their pipes
/// instead of the listener (which still takes observers and the other robots).
pub fn run_battle_piped(
    listener: TcpListener,
    observers: Option<Observers>,
    config: &ServerConfig,
    robots: Vec<RobotEntry>,
    pipes: Vec<RobotPipe>,
) -> io::Result<BattleOutcome> {
    run_battle_remote(listener, observers, config, robots, pipes, None)
}

/// [`run_battle_piped`], where the robots `agents` expects ([`AgentListener::expect`])
/// connect to it, through their runner agents, instead of the listener.
pub fn run_battle_remote(
    listener: TcpListener,
    observers: Option<Observers>,
    config: &ServerConfig,
    robots: Vec<RobotEntry>,
    pipes: Vec<RobotPipe>,
    agents: Option<&AgentListener>,
) -> io::Result<BattleOutcome> {
    if let Some(o) = &observers {
        net::check_listener(&o.listener, o.tls.as_ref())?;
    }
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let tokens: Vec<String> = robots.iter().map(|r| r.token.clone()).collect();
    // A piped or remote robot's index isn't open to the listener, whatever token it
    // presents.
    let mut taken = vec![false; tokens.len()];
    for pipe in pipes {
        if let Some(t) = taken.get_mut(pipe.robot) {
            *t = true;
        }
        spawn_pipe_robot(pipe, tx.clone());
    }
    if let Some(agents) = agents {
        for index in agents.attach(tx.clone()) {
            if let Some(t) = taken.get_mut(index) {
                *t = true;
            }
        }
    }
    let acceptor = {
        let (tx, stop) = (tx.clone(), stop.clone());
        let observer_token = config.observer_token.clone();
        listener.set_nonblocking(true)?;
        thread::spawn(move || accept_loop(&listener, &tokens, taken, &observer_token, &tx, &stop))
    };
    let wait = observers.as_ref().is_some_and(|o| o.wait);
    let observer_acceptor = match observers {
        Some(o) => {
            let (tx, stop) = (tx.clone(), stop.clone());
            o.listener.set_nonblocking(true)?;
            Some(thread::spawn(move || {
                observer_accept_loop(&o.listener, &o.token, o.tls.as_ref(), &tx, &stop)
            }))
        }
        None => None,
    };
    drop(tx);
    let result = BattleLoop::new(config, robots, rx).and_then(|mut b| {
        b.await_observer = wait;
        b.run()
    });
    stop.store(true, Ordering::SeqCst);
    if let Some(agents) = agents {
        agents.detach();
    }
    let _ = acceptor.join();
    if let Some(a) = observer_acceptor {
        let _ = a.join();
    }
    result
}

/// Accepts observers on an [`Observers`] listener, each handshake on its own thread (a
/// slow one doesn't hold up the next), at most [`MAX_REMOTE_OBSERVERS`] at once.
fn observer_accept_loop(
    listener: &TcpListener,
    token: &str,
    tls: Option<&tls::Identity>,
    tx: &Sender<Incoming>,
    stop: &AtomicBool,
) {
    let connected = Arc::new(AtomicUsize::new(0));
    let handshakes = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            // Connections that haven't shown the token yet don't each get a thread without
            // end: past the limit, they are closed at once.
            Ok((stream, _)) if handshakes.load(Ordering::SeqCst) >= MAX_HANDSHAKES => {
                drop(stream);
            }
            Ok((stream, _)) => {
                let (token, tx, connected) = (token.to_string(), tx.clone(), connected.clone());
                let (handshakes, tls) = (handshakes.clone(), tls.cloned());
                handshakes.fetch_add(1, Ordering::SeqCst);
                thread::spawn(move || {
                    let _ = Stream::accept(stream, tls.as_ref(), HELLO_TIMEOUT)
                        .and_then(|stream| observer_handshake(stream, &token, &tx, &connected));
                    handshakes.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Reads an observer's `Hello` on an [`Observers`] listener: only observers, with the
/// token.
fn observer_handshake(
    mut stream: Stream,
    token: &str,
    tx: &Sender<Incoming>,
    connected: &Arc<AtomicUsize>,
) -> io::Result<()> {
    let hello = match read_hello(&mut stream)? {
        Some(v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(h)),
        }) => h,
        _ => return refuse(stream, "expected Hello"),
    };
    if hello.version.map(|v| v.major) != Some(PROTOCOL_VERSION.major) {
        return refuse(stream, "unsupported protocol version");
    }
    if v1::Role::try_from(hello.role) != Ok(v1::Role::Observer) {
        return refuse(stream, "only observers connect here");
    }
    if !same_secret(hello.token.as_bytes(), token.as_bytes()) {
        // A moment's delay makes guessing slower.
        thread::sleep(Duration::from_millis(500));
        return refuse(stream, "wrong observer token");
    }
    if connected.fetch_add(1, Ordering::SeqCst) >= MAX_REMOTE_OBSERVERS {
        connected.fetch_sub(1, Ordering::SeqCst);
        return refuse(stream, "too many observers");
    }
    stream.set_write_timeout(Some(REMOTE_OBSERVER_WRITE_TIMEOUT))?;
    let reader = reader_of(&stream)?;
    let (tx2, connected) = (tx.clone(), connected.clone());
    spawn_observer_reader(reader, tx.clone(), move || {
        connected.fetch_sub(1, Ordering::SeqCst);
        let _ = tx2.send(Incoming::ObserverLeft { remote: true });
    });
    let _ = tx.send(Incoming::ObserverConnected {
        stream,
        remote: true,
    });
    Ok(())
}

/// Compares a presented token with the secret in time that doesn't depend on where they
/// differ.
pub(crate) fn same_secret(presented: &[u8], secret: &[u8]) -> bool {
    presented.len() == secret.len()
        && presented
            .iter()
            .zip(secret)
            .fold(0u8, |d, (a, b)| d | (a ^ b))
            == 0
}

fn accept_loop(
    listener: &TcpListener,
    tokens: &[String],
    mut taken: Vec<bool>,
    observer_token: &str,
    tx: &Sender<Incoming>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handshake(stream, tokens, observer_token, &mut taken, tx);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Reads the `Hello` and hands the connection to the battle loop. Robots and observers
/// alike show their token: the listener is on loopback, but any local process reaches it.
fn handshake(
    stream: TcpStream,
    tokens: &[String],
    observer_token: &str,
    taken: &mut [bool],
    tx: &Sender<Incoming>,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    let mut stream = Stream::plain(stream);
    let hello = match read_hello(&mut stream)? {
        Some(v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(h)),
        }) => h,
        _ => return refuse(stream, "expected Hello"),
    };
    if hello.version.map(|v| v.major) != Some(PROTOCOL_VERSION.major) {
        return refuse(stream, "unsupported protocol version");
    }
    let reader = reader_of(&stream)?;
    match v1::Role::try_from(hello.role) {
        Ok(v1::Role::Robot) => {
            // Every token is compared, in constant time each.
            let index = tokens.iter().enumerate().fold(None, |found, (i, t)| {
                let matches = same_secret(hello.token.as_bytes(), t.as_bytes());
                found.or(matches.then_some(i))
            });
            let Some(index) = index else {
                return refuse(stream, "unknown robot token");
            };
            if taken[index] {
                return refuse(stream, "robot token already used");
            }
            taken[index] = true;
            spawn_robot_reader(index, reader, tx.clone());
            let stream = Conn::Tcp(stream);
            let _ = tx.send(Incoming::RobotConnected { index, stream });
        }
        Ok(v1::Role::Observer) => {
            if !same_secret(hello.token.as_bytes(), observer_token.as_bytes()) {
                return refuse(stream, "wrong observer token");
            }
            let left = tx.clone();
            spawn_observer_reader(reader, tx.clone(), move || {
                let _ = left.send(Incoming::ObserverLeft { remote: false });
            });
            let _ = tx.send(Incoming::ObserverConnected {
                stream,
                remote: false,
            });
        }
        _ => return refuse(stream, "unsupported role"),
    }
    Ok(())
}

/// How long a new connection has to say `Hello`.
pub(crate) const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Reads a new connection's first message, the `Hello`, within [`HELLO_TIMEOUT`]; reads
/// wait as long as it takes afterwards.
pub(crate) fn read_hello(stream: &mut Stream) -> io::Result<Option<v1::ClientMessage>> {
    stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
    let hello = read_message::<v1::ClientMessage>(stream);
    stream.set_read_timeout(None)?;
    hello
}

/// A handle on `stream` for its reader thread, which waits as long as it takes for the
/// next message.
///
/// On Windows a clone is a socket handle of its own (`WSADuplicateSocket`) with timeouts
/// of its own: a reader cloned during the handshake kept its 10 s timeout, so it ended
/// after 10 s without a message (a GUI on another tab sends no controls, robots in a
/// paused battle no `Execute`). The battle went on, still sending snapshots, deaf to
/// that observer's speed, pause and stop.
pub(crate) fn reader_of(stream: &Stream) -> io::Result<Stream> {
    let reader = stream.try_clone()?;
    reader.set_read_timeout(None)?;
    Ok(reader)
}

pub(crate) fn refuse(mut stream: Stream, message: &str) -> io::Result<()> {
    write_message(
        &mut stream,
        &server(server_message::Msg::Error(v1::Error {
            message: message.into(),
        })),
    )?;
    stream.shutdown()
}

pub(crate) fn spawn_robot_reader(robot: usize, reader: Stream, tx: Sender<Incoming>) {
    thread::spawn(move || read_robot(robot, reader, &tx));
}

/// Takes a piped robot's `Hello`, hands its connection to the battle loop and goes on
/// reading it. A pipe has no read timeout, so a robot that never says `Hello` holds this
/// thread, not the battle (which goes on after its connect timeout): the thread ends when
/// the runner kills the process.
fn spawn_pipe_robot(pipe: RobotPipe, tx: Sender<Incoming>) {
    thread::spawn(move || {
        let RobotPipe {
            robot,
            mut reader,
            writer,
        } = pipe;
        let mut conn = Conn::Pipe(Some(writer));
        let hello = read_message::<v1::ClientMessage>(&mut reader);
        let ok = matches!(
            &hello,
            Ok(Some(v1::ClientMessage { msg: Some(client_message::Msg::Hello(h)) }))
                if h.version.map(|v| v.major) == Some(PROTOCOL_VERSION.major)
                    && h.role == v1::Role::Robot as i32
        );
        if !ok {
            let message = match hello {
                Ok(Some(_)) => "expected a robot Hello of this protocol version".into(),
                other => why_ended(&other),
            };
            let error = v1::Error { message };
            let _ = write_message(&mut conn, &server(server_message::Msg::Error(error)));
            let _ = conn.flush();
            conn.shutdown();
            let _ = tx.send(Incoming::RobotDisconnected { robot });
            return;
        }
        if tx
            .send(Incoming::RobotConnected {
                index: robot,
                stream: conn,
            })
            .is_ok()
        {
            read_robot(robot, reader, &tx);
        }
    });
}

/// Forwards a robot's messages to the battle loop until its connection ends.
fn read_robot(robot: usize, mut reader: impl Read, tx: &Sender<Incoming>) {
    loop {
        match read_message::<v1::ClientMessage>(&mut reader) {
            Ok(Some(v1::ClientMessage { msg: Some(message) })) => {
                let message = Box::new(message);
                if tx.send(Incoming::Message { robot, message }).is_err() {
                    return;
                }
            }
            Ok(Some(_)) => {}
            ended => {
                log_perf(format_args!(
                    "server drops robot {robot} because {}",
                    why_ended(&ended)
                ));
                let _ = tx.send(Incoming::RobotDisconnected { robot });
                return;
            }
        }
    }
}

/// Forwards an observer's controls; `left` runs when its connection ends.
///
/// An observer whose controls can't be read anymore is cut off altogether (its
/// connection shut down): one that still got the turns would look alive while its
/// speed, pause and stop went nowhere.
fn spawn_observer_reader(
    mut reader: Stream,
    tx: Sender<Incoming>,
    left: impl FnOnce() + Send + 'static,
) {
    thread::spawn(move || {
        let ended = loop {
            match read_message::<v1::ClientMessage>(&mut reader) {
                Ok(Some(message)) => {
                    if let Some(client_message::Msg::ObserverControl(c)) = message.msg
                        && tx.send(Incoming::Control(c)).is_err()
                    {
                        return;
                    }
                }
                ended => break ended,
            }
        };
        log_perf(format_args!(
            "server drops observer because {}",
            why_ended(&ended)
        ));
        let _ = reader.shutdown();
        left();
    });
}

/// Why a reader thread's connection ended, for the perf log.
fn why_ended<T>(ended: &io::Result<Option<T>>) -> String {
    match ended {
        Ok(Some(_)) => "it sent a message it shouldn't".into(),
        Ok(None) => "its connection was closed".into(),
        Err(e) => format!("reading from it failed: {e}"),
    }
}

/// Whether `ROBOCODES_PERF_LOG` is on (as the GUI's).
fn perf_log_on() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| {
        std::env::var("ROBOCODES_PERF_LOG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// `ROBOCODES_PERF_LOG`: logs `what` to stderr, as `[perf] what`.
pub fn log_perf(what: std::fmt::Arguments) {
    if perf_log_on() {
        eprintln!("[perf] {what}");
    }
}

/// `ROBOCODES_PERF_LOG` (as the GUI's): logs the observers' controls (but their mouse
/// and keyboard events) to stderr, `what` saying where.
pub fn log_control(what: &str, control: &v1::ObserverControl) {
    if perf_log_on()
        && !matches!(
            control.control,
            Some(v1::observer_control::Control::Input(_))
        )
    {
        eprintln!("[perf] {what} {:?}", control.control);
    }
}

pub(crate) fn server(msg: server_message::Msg) -> v1::ServerMessage {
    v1::ServerMessage { msg: Some(msg) }
}

struct BattleLoop<'a> {
    config: &'a ServerConfig,
    statics: Vec<v1::RobotStatics>,
    /// The engine; every input goes through it and into the recording.
    engine: RecordingDriver,
    links: Vec<Link>,
    observers: Vec<Stream>,
    rx: Receiver<Incoming>,
    /// What came from the robots while the battle waited between turns, for the next
    /// turn's wait for them: they answer as before, after that turn's step.
    held: VecDeque<Incoming>,
    paused: bool,
    steps: u32,
    turns_per_second: u32,
    /// When the last paced turn was due: the next one is due a turn's time later.
    paced: Option<Instant>,
    /// An observer stopped the battle; the engine aborts before its next step.
    abort: bool,
    /// Robots an observer killed; they die before the next step.
    kills: Vec<usize>,
    /// Observers' mouse and keyboard events, delivered with the next step's events.
    input: Vec<v1::InputEvent>,
    /// The battle waits for an observer from an [`Observers`] listener before it starts.
    await_observer: bool,
    /// Observers from an [`Observers`] listener connected now.
    remote_observers: usize,
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
                    engine_version: robocode_s_engine::ENGINE_VERSION,
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
            held: VecDeque::new(),
            paused: false,
            steps: 0,
            turns_per_second: 0,
            paced: None,
            abort: false,
            kills: Vec::new(),
            input: Vec::new(),
            await_observer: false,
            remote_observers: 0,
        })
    }

    fn run(mut self) -> io::Result<BattleOutcome> {
        self.await_robots();
        while self.await_observer && !self.abort {
            match self.rx.recv() {
                Ok(incoming) => self.handle(incoming),
                Err(_) => break,
            }
        }
        let mut rounds = 0;
        // As classic: no round starts after a stop, one that came since the last turn too.
        while rounds < self.config.rules.num_rounds && !self.abort {
            self.run_round();
            self.engine.input(Entry::FinishRound(true));
            rounds += 1;
            self.take_controls(None);
        }
        let aborted = self.abort;
        for i in 0..self.links.len() {
            self.send(
                i,
                server_message::Msg::BattleOver(v1::BattleOver {
                    aborted,
                    ..Default::default()
                }),
            );
        }
        let results = recording::round_result(self.battle()).results;
        self.broadcast(server_message::Msg::BattleOver(v1::BattleOver {
            aborted,
            rounds,
            results: results.clone(),
        }));
        // Reader threads hold clones of the sockets; shutting them down closes the
        // connections and ends those threads. A piped robot's stdin closes: it ends.
        for stream in self.links.iter_mut().filter_map(|l| l.stream.as_mut()) {
            stream.shutdown();
        }
        for stream in &self.observers {
            let _ = stream.shutdown();
        }
        let finished = self.engine.finish();
        // A battle stopped before its first round (a cancelled start) leaves no
        // recording. The file is created up front, so that a bad path fails at once.
        if rounds == 0
            && let Some(path) = &self.config.record
        {
            let _ = std::fs::remove_file(path);
        }
        finished?;
        Ok(BattleOutcome {
            results,
            rounds,
            aborted,
        })
    }

    /// Waits until every robot connected, or the connect timeout passed.
    fn await_robots(&mut self) {
        let deadline = Instant::now() + self.config.connect_timeout;
        while self.links.iter().any(|l| l.stream.is_none()) && !self.abort {
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
        // A kill from the last round's end doesn't reach into this one.
        self.kills.clear();
        self.input.clear();
        self.engine.input(Entry::StartRound(true));
        let round = self.battle().round;
        for i in 0..self.links.len() {
            let link = &mut self.links[i];
            link.events.clear();
            link.input.clear();
            link.bullet_updates.clear();
            link.team_messages.clear();
            link.awaiting = None;
            link.active = false;
            link.final_execute = false;
            link.late_execute = None;
            link.answered_late = None;
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
            if self.abort && !self.battle().aborted {
                self.engine.input(Entry::Abort(true));
            }
            for i in std::mem::take(&mut self.kills) {
                if self.battle().robots[i].is_alive() {
                    self.engine.input(Entry::Kill(i as i32));
                }
            }
            for input in std::mem::take(&mut self.input) {
                self.queue_input(input);
            }
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

    /// Records an observer's mouse or keyboard event and queues it for every running
    /// interactive robot, as classic's `SendInteractiveEventCommand` (`addEvent` adds only
    /// to running robots). It arrives with the next step's events.
    fn queue_input(&mut self, input: v1::InputEvent) {
        self.engine.input(Entry::Input(input));
        let event = v1::Event {
            kind: Some(v1::event::Kind::Input(input)),
        };
        for i in 0..self.links.len() {
            let interactive = self.statics[i].r#type.is_some_and(|t| t.interactive);
            if interactive && self.links[i].active && self.battle().robots[i].is_running() {
                self.links[i].input.push(event.clone());
            }
        }
    }

    fn battle(&self) -> &Battle {
        &self.engine.driver.battle
    }

    /// Whether we still expect an `Execute` from this robot before the next turn.
    fn expecting(&self, i: usize) -> bool {
        let link = &self.links[i];
        link.awaiting.is_some()
            && link.late_execute.is_none()
            && link.stream.is_some()
            && (self.battle().robots[i].is_running() || link.final_execute)
    }

    /// Waits for the `Execute`s of this turn (engine-spec §11.3).
    fn collect_executes(&mut self) {
        let timeout = if self.abort {
            // Stopped: robots that are slow to answer skip the turn, in either mode.
            self.config.turn_timeout.min(self.config.hard_limit)
        } else if self.config.deterministic {
            self.config.hard_limit
        } else if self.battle().time == 0 {
            self.config.start_timeout
        } else {
            self.config.turn_timeout
        };
        while let Some(incoming) = self.held.pop_front() {
            self.handle(incoming);
        }
        // Late `Execute`s arrive now: the robots skipped the turns in between.
        let time = self.battle().time;
        for i in 0..self.links.len() {
            if self.links[i]
                .late_execute
                .as_ref()
                .is_some_and(|(due, _)| *due <= time)
            {
                let (_, execute) = self.links[i].late_execute.take().unwrap();
                self.apply_execute(i, execute);
            }
        }
        let mut deadline = Instant::now() + timeout;
        while (0..self.links.len()).any(|i| self.expecting(i)) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match self.rx.recv_timeout(left) {
                Ok(incoming) => {
                    self.handle(incoming);
                    if self.abort {
                        deadline = deadline.min(Instant::now() + self.config.turn_timeout);
                    }
                }
                Err(_) => break,
            }
        }
        if self.config.deterministic && !self.abort {
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
            Incoming::ObserverConnected { mut stream, remote } => {
                let info = v1::BattleInfo {
                    rules: Some(convert::rules(
                        &self.config.rules,
                        self.config.deterministic,
                    )),
                    robots: self.statics.clone(),
                    seed: self.config.seed,
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
                    if remote {
                        self.await_observer = false;
                        self.remote_observers += 1;
                    }
                } else {
                    let _ = stream.shutdown();
                }
            }
            // An observer that paused the battle and left doesn't hold it up for the others
            // (such as the command line's own). Once the last one from elsewhere left, the
            // battle runs at full speed again, as it did before them.
            //
            // The battle's own observer (the GUI's or the command line's, which started
            // it) leaving means its connection broke: nobody would see or control the
            // battle anymore, so it stops.
            Incoming::ObserverLeft { remote } => {
                self.paused = false;
                self.steps = 0;
                if !remote {
                    self.abort = true;
                }
                if remote {
                    self.remote_observers = self.remote_observers.saturating_sub(1);
                    if self.remote_observers == 0 {
                        self.turns_per_second = 0;
                    }
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
                self.links[robot].late_execute = None;
            }
        }
    }

    fn on_execute(&mut self, i: usize, execute: v1::Execute) {
        let link = &self.links[i];
        if link.awaiting != Some(execute.turn) || link.late_execute.is_some() {
            return; // not the reply we wait for
        }
        if self.config.deterministic && !link.final_execute && !execute.stopped {
            // The robot's counted work decides when it answers (classic: its wall time). Not
            // after its code ended: classic's `waitForBattleEndImpl` isn't waited for.
            let late = late_turns(execute.cpu_nanos, execute.turn, DETERMINISTIC_BUDGET_NANOS);
            if late > 0 {
                self.links[i].late_execute = Some((execute.turn + late, execute));
                return;
            }
        }
        self.apply_execute(i, execute);
    }

    /// An `Execute` arrives: as classic's `executeImpl` and `checkSkippedTurn`.
    fn apply_execute(&mut self, i: usize, execute: v1::Execute) {
        let link = &mut self.links[i];
        link.awaiting = None;
        link.final_execute = false;
        link.paint = execute.paint;
        link.cpu_nanos = link.cpu_nanos.saturating_add(execute.cpu_nanos);
        // Only for observers and recordings (which take them with the next snapshot);
        // without either, nothing takes them and they would pile up.
        if !self.observers.is_empty() || self.engine.is_recording() {
            link.output.push_str(&execute.output);
            link.debug_properties.extend(execute.debug_properties);
        }
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
        // Turns ran since the Tick this robot answers: it skipped them, which it learns
        // when it gets its next Tick (`deliver`).
        if self.battle().time > execute.turn {
            self.links[i].answered_late = Some(execute.turn);
        }
    }

    fn on_control(&mut self, control: v1::ObserverControl) {
        log_control("server applies", &control);
        match control.control {
            Some(v1::observer_control::Control::Pause(pause)) => self.paused = pause,
            Some(v1::observer_control::Control::Step(true)) => self.steps += 1,
            Some(v1::observer_control::Control::TurnsPerSecond(tps)) => self.turns_per_second = tps,
            Some(v1::observer_control::Control::Abort(true)) => self.abort = true,
            Some(v1::observer_control::Control::Kill(robot)) => {
                if let Some(i) = usize::try_from(robot)
                    .ok()
                    .filter(|&i| i < self.links.len())
                {
                    self.kills.push(i);
                }
            }
            Some(v1::observer_control::Control::Input(input)) => {
                // At most one move and one drag a turn: a newer one replaces the last.
                let coalesced = |e: &v1::InputEvent| match &e.kind {
                    Some(v1::input_event::Kind::Mouse(m)) => matches!(
                        m.action(),
                        v1::MouseAction::MouseMoved | v1::MouseAction::MouseDragged
                    )
                    .then(|| m.action()),
                    _ => None,
                };
                if let Some(action) = coalesced(&input)
                    && let Some(last) = self.input.last_mut()
                    && coalesced(last) == Some(action)
                {
                    *last = input;
                } else {
                    self.input.push(input);
                }
            }
            Some(v1::observer_control::Control::Paint(toggle)) => {
                if let Some(link) = usize::try_from(toggle.robot_index)
                    .ok()
                    .and_then(|i| self.links.get_mut(i))
                {
                    link.paint_enabled = toggle.enabled;
                }
            }
            _ => {}
        }
    }

    /// Sends each robot that answered its last `Tick` the results of this turn. A robot
    /// that is still busy (skipping turns) gets them, accumulated, after its `Execute`.
    fn deliver(&mut self) {
        let turn = self.battle().time;
        let round_over = self.battle().is_round_over();
        let budget = self.budget_nanos();
        for i in 0..self.links.len() {
            // `checkSkippedTurn`, when classic wakes a robot that answered late: after this
            // turn ran, so this turn's events are cleared too. Then the skipped turns' events,
            // then the limit.
            let link = &mut self.links[i];
            let answered_late = if link.active && link.stream.is_some() && link.awaiting.is_none() {
                link.answered_late.take()
            } else {
                None
            };
            let skipped_alive = self.battle().robots[i].is_alive();
            if let Some(answered) = answered_late {
                let max = if self.links[i].io_robot {
                    MAX_SKIPPED_TURNS_WITH_IO
                } else {
                    MAX_SKIPPED_TURNS
                };
                if turn - answered - 1 > max {
                    self.engine.input(Entry::DiscardCommands(i as i32));
                    self.engine.input(Entry::Disable(i as i32));
                    // Classic then stops the robot's thread: its code doesn't run again.
                    self.engine.input(Entry::Stop(i as i32));
                }
            }
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
            if let Some(answered) = answered_late {
                link.events.retain(convert::is_critical);
                link.input.clear();
                if skipped_alive {
                    for skipped in answered + 1..turn {
                        link.events.push(Event::SkippedTurn { turn: skipped });
                    }
                }
            }
            if link.stream.is_none() || link.awaiting.is_some() {
                continue;
            }
            let tick = v1::Tick {
                turn,
                status: Some(status),
                commands: Some(commands),
                events: link
                    .input
                    .drain(..)
                    .chain(link.events.drain(..).map(|e| convert::event(&e, &name)))
                    .collect(),
                team_messages: std::mem::take(&mut link.team_messages),
                bullet_updates: std::mem::take(&mut link.bullet_updates),
                halt: !running,
                should_wait,
                paint_enabled: link.paint_enabled,
                budget_nanos: budget,
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

    /// A robot's CPU budget per turn; in deterministic mode, of counted work.
    fn budget_nanos(&self) -> u64 {
        if self.config.deterministic {
            DETERMINISTIC_BUDGET_NANOS
        } else {
            self.config.turn_timeout.as_nanos() as u64
        }
    }

    /// Broadcasts the current turn; `with_output` takes the robots' console output so far.
    fn snapshot_with(&mut self, with_output: bool) {
        if self.observers.is_empty() && !self.engine.is_recording() {
            return;
        }
        let links = &mut self.links;
        let mut snapshot = self.engine.driver.snapshot(&mut |i| {
            if with_output {
                std::mem::take(&mut links[i].output)
            } else {
                String::new()
            }
        });
        let check = recording::check(&snapshot);
        let budget = self.budget_nanos();
        for (r, link) in snapshot.robots.iter_mut().zip(&mut self.links) {
            if with_output {
                r.paint = std::mem::take(&mut link.paint);
                r.cpu_nanos = std::mem::take(&mut link.cpu_nanos);
                r.debug_properties = std::mem::take(&mut link.debug_properties);
            }
            r.budget_nanos = budget;
        }
        if self.engine.is_recording() {
            let marker = v1::RecordedSnapshot {
                output: snapshot.robots.iter().map(|r| r.output.clone()).collect(),
                paint: snapshot
                    .robots
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !r.paint.is_empty())
                    .map(|(i, r)| v1::RobotPaint {
                        robot: i as i32,
                        ops: r.paint.clone(),
                    })
                    .collect(),
                check,
                cpu_nanos: snapshot.robots.iter().map(|r| r.cpu_nanos).collect(),
                budget_nanos: budget,
                debug_properties: snapshot
                    .robots
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !r.debug_properties.is_empty())
                    .map(|(i, r)| v1::RobotDebugProperties {
                        robot: i as i32,
                        properties: r.debug_properties.clone(),
                    })
                    .collect(),
            };
            self.engine.record(&Entry::Snapshot(marker));
        }
        self.broadcast(server_message::Msg::TurnSnapshot(snapshot));
    }

    /// Observer pacing: pause, single steps and a turn rate. A stopped battle runs out its
    /// round at full speed (classic neither pauses nor delays an aborted battle).
    ///
    /// The observers' controls are taken in here as well, between turns: the wait for the
    /// robots' answers takes them in too, but robots that no longer run (a `run` that
    /// returned, a crash) are not waited for, and with none running a round went on at the
    /// old speed, deaf to speed, pause and stop until it ended.
    fn pace(&mut self) {
        self.take_controls(None);
        if self.abort {
            self.paced = None;
            return;
        }
        while self.paused && self.steps == 0 && !self.observers.is_empty() && !self.abort {
            self.paced = None;
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(incoming) => self.take(incoming),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.paused && self.steps > 0 {
            self.steps -= 1;
        }
        // Turns are due a turn's time apart, whatever the turns themselves take: sleeping
        // a whole turn's time after each would add their time (and the sleep's overshoot)
        // to every turn, and fall well short of fast rates (1000 a second ran at 600).
        // A battle that fell behind (slow robots, a pause, a faster rate) starts afresh
        // rather than catching up in a burst. Controls that come while it waits apply at
        // once: a faster rate (or a stop) doesn't wait out the slow rate's turn.
        loop {
            if self.turns_per_second == 0 || self.abort {
                self.paced = None;
                return;
            }
            let interval = Duration::from_secs_f64(1.0 / f64::from(self.turns_per_second));
            let now = Instant::now();
            let due = self
                .paced
                .map(|last| last + interval)
                .filter(|&due| due + interval >= now)
                .unwrap_or(now);
            match due.checked_duration_since(now).filter(|w| !w.is_zero()) {
                Some(wait) if self.take_controls(Some(wait)) => {}
                Some(_) | None => {
                    self.paced = Some(due);
                    return;
                }
            }
        }
    }

    /// Takes in what has come, or waits up to `wait` for something to come: observers'
    /// controls apply now, the robots' messages are held for the next turn. True when
    /// something came before `wait` was over.
    fn take_controls(&mut self, wait: Option<Duration>) -> bool {
        let incoming = match wait {
            Some(wait) => match self.rx.recv_timeout(wait) {
                Ok(incoming) => incoming,
                Err(RecvTimeoutError::Timeout) => return false,
                Err(RecvTimeoutError::Disconnected) => {
                    // Nothing comes anymore: wait out the turn.
                    thread::sleep(wait);
                    return false;
                }
            },
            None => {
                while let Ok(incoming) = self.rx.try_recv() {
                    self.take(incoming);
                }
                return false;
            }
        };
        self.take(incoming);
        true
    }

    /// An observer's control or connection now, anything else with the next turn.
    fn take(&mut self, incoming: Incoming) {
        match incoming {
            Incoming::Control(_)
            | Incoming::ObserverConnected { .. }
            | Incoming::ObserverLeft { .. } => self.handle(incoming),
            other => self.held.push_back(other),
        }
    }

    fn send(&mut self, i: usize, msg: server_message::Msg) {
        if let Some(stream) = self.links[i].stream.as_mut() {
            if let Err(e) = write_message(stream, &server(msg)).and_then(|()| stream.flush()) {
                log_perf(format_args!(
                    "server drops robot {i} because writing to it failed: {e}"
                ));
                self.links[i].stream = None;
                self.links[i].awaiting = None;
            }
        }
    }

    fn broadcast(&mut self, msg: server_message::Msg) {
        let message = server(msg);
        // A failed (or timed out) write leaves the stream mid-message: shut it down, which
        // also ends its reader thread and so tells the battle the observer left.
        self.observers
            .retain_mut(|o| match write_message(o, &message) {
                Ok(()) => true,
                Err(e) => {
                    log_perf(format_args!(
                        "server drops observer because writing to it failed: {e}"
                    ));
                    let _ = o.shutdown();
                    false
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_reader_waits_as_long_as_it_takes_after_the_hello() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut stream = Stream::plain(listener.accept().unwrap().0);
        let hello = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(v1::Hello::default())),
        };
        write_message(&mut client, &hello).unwrap();
        assert!(read_hello(&mut stream).unwrap().is_some());
        assert_eq!(stream.read_timeout().unwrap(), None);
        // On Windows a clone has timeouts of its own: one taken with the hello's still
        // set kept it, and its reader ended after 10 s without a message.
        stream.set_read_timeout(Some(HELLO_TIMEOUT)).unwrap();
        let reader = reader_of(&stream).unwrap();
        assert_eq!(reader.read_timeout().unwrap(), None);
    }
}
