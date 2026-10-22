// SPDX-License-Identifier: EPL-2.0

//! Runner agents (ADR 0020): `robocode-s agent` on another machine connects here with the
//! agent token (role runner), and the robots it starts for a battle connect here too,
//! each with its one-time token. TLS unless the listener is on loopback ([`crate::net`]).
//!
//! The battle machine takes the agents that are here ([`AgentListener::take_agents`]),
//! tells the listener which robots to expect ([`AgentListener::expect`]) and asks the
//! agents to start them (`runner::remote`); robots that connect before the battle runs
//! wait in a queue until it does ([`crate::run_battle_remote`]).

use crate::net::{self, Stream};
use crate::tls::{Fingerprint, Identity};
use crate::{
    Conn, HELLO_TIMEOUT, Incoming, read_hello, reader_of, refuse, same_secret, server,
    spawn_robot_reader,
};
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// At most this many handshakes (up to the token check) at once.
const MAX_HANDSHAKES: usize = 64;

/// The listener for runner agents and their robots. Clones share it.
#[derive(Clone)]
pub struct AgentListener {
    inner: Arc<Inner>,
}

struct Inner {
    address: SocketAddr,
    fingerprint: Option<Fingerprint>,
    stop: AtomicBool,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// Agents here and waiting for work.
    idle: Vec<Agent>,
    /// The robots of the battle: index and one-time token.
    expected: Vec<(usize, String)>,
    /// Indexes whose robot connected.
    taken: Vec<usize>,
    /// Robots that connected before the battle runs.
    queued: Vec<(usize, Stream)>,
    /// The running battle.
    tx: Option<Sender<Incoming>>,
    /// The robots' connections, to end one whose agent left.
    live: Vec<(usize, Stream)>,
}

/// A runner agent that said `Hello` with the agent token.
pub struct Agent {
    id: u64,
    pub info: v1::RunnerInfo,
    pub peer: SocketAddr,
    stream: Stream,
    /// Its messages, then [`AgentEvent::Gone`].
    pub events: Receiver<AgentEvent>,
}

/// What comes from an agent.
#[derive(Debug)]
pub enum AgentEvent {
    Message(Box<client_message::Msg>),
    /// Its connection ended.
    Gone(String),
}

impl Agent {
    pub fn send(&mut self, msg: server_message::Msg) -> io::Result<()> {
        write_message(&mut self.stream, &server(msg))
    }

    /// Ends its connection (the agent then starts over).
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown();
    }
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Agent({}, {})", self.peer, self.info.platform)
    }
}

impl AgentListener {
    /// Listens on `address` for agents presenting `token`, with TLS when `tls` is given
    /// (required off loopback).
    pub fn bind(address: &str, tls: Option<Identity>, token: String) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        net::check_listener(&listener, tls.as_ref())?;
        let inner = Arc::new(Inner {
            address: listener.local_addr()?,
            fingerprint: tls.as_ref().map(|t| t.fingerprint),
            stop: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        listener.set_nonblocking(true)?;
        let shared = inner.clone();
        thread::spawn(move || accept_loop(&listener, &shared, tls, &token));
        Ok(Self { inner })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.address
    }

    /// The certificate's fingerprint; `None` for a plain (loopback) listener.
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.inner.fingerprint
    }

    /// Stops listening.
    pub fn close(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
    }

    /// Takes every agent here once at least `at_least` are, waiting up to `timeout` for
    /// them; `Err` with how many there are after that.
    pub fn take_agents(&self, at_least: usize, timeout: Duration) -> Result<Vec<Agent>, usize> {
        let deadline = Instant::now() + timeout;
        let mut state = self.inner.state.lock().unwrap();
        while state.idle.len() < at_least.max(1) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(state.idle.len());
            };
            state = self.inner.changed.wait_timeout(state, left).unwrap().0;
        }
        Ok(std::mem::take(&mut state.idle))
    }

    /// The robots (index, one-time token) to take for the next battle.
    pub fn expect(&self, robots: Vec<(usize, String)>) {
        let mut state = self.inner.state.lock().unwrap();
        state.expected = robots;
        state.taken.clear();
        state.queued.clear();
        state.live.clear();
    }

    /// Ends the connection of robot `index` (its agent left): the battle sees it go.
    pub fn disconnect(&self, index: usize) {
        let state = self.inner.state.lock().unwrap();
        for (i, stream) in state.live.iter().chain(&state.queued) {
            if *i == index {
                let _ = stream.shutdown();
            }
        }
    }

    /// The battle starts: robots go to `tx` from now on, those queued first. The indexes
    /// expected.
    pub(crate) fn attach(&self, tx: Sender<Incoming>) -> Vec<usize> {
        let mut state = self.inner.state.lock().unwrap();
        for (index, stream) in std::mem::take(&mut state.queued) {
            if let Some(live) = connect_robot(index, stream, &tx) {
                state.live.push((index, live));
            }
        }
        state.tx = Some(tx);
        state.expected.iter().map(|(i, _)| *i).collect()
    }

    /// The battle is over: no robots are expected anymore.
    pub(crate) fn detach(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.tx = None;
        state.expected.clear();
        state.queued.clear();
        state.live.clear();
    }
}

/// Hands a robot's connection to the battle; a handle to end it.
fn connect_robot(index: usize, stream: Stream, tx: &Sender<Incoming>) -> Option<Stream> {
    let reader = reader_of(&stream).ok()?;
    let handle = stream.try_clone().ok()?;
    spawn_robot_reader(index, reader, tx.clone());
    let _ = tx.send(Incoming::RobotConnected {
        index,
        stream: Conn::Tcp(stream),
    });
    Some(handle)
}

fn accept_loop(listener: &TcpListener, inner: &Arc<Inner>, tls: Option<Identity>, token: &str) {
    let handshakes = Arc::new(AtomicUsize::new(0));
    while !inner.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((socket, _)) if handshakes.load(Ordering::SeqCst) >= MAX_HANDSHAKES => {
                drop(socket);
            }
            Ok((socket, _)) => {
                let (inner, tls, token) = (inner.clone(), tls.clone(), token.to_string());
                let handshakes = handshakes.clone();
                handshakes.fetch_add(1, Ordering::SeqCst);
                thread::spawn(move || {
                    if let Ok(stream) = Stream::accept(socket, tls.as_ref(), HELLO_TIMEOUT) {
                        let _ = handshake(stream, &inner, &token);
                    }
                    handshakes.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Reads the `Hello` of an agent or of one of its robots.
fn handshake(mut stream: Stream, inner: &Arc<Inner>, token: &str) -> io::Result<()> {
    let hello = match read_hello(&mut stream)? {
        Some(v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(h)),
        }) => h,
        _ => return refuse(stream, "expected Hello"),
    };
    if hello.version.map(|v| v.major) != Some(PROTOCOL_VERSION.major) {
        return refuse(stream, "unsupported protocol version");
    }
    match v1::Role::try_from(hello.role) {
        Ok(v1::Role::Runner) => {
            if !same_secret(hello.token.as_bytes(), token.as_bytes()) {
                thread::sleep(Duration::from_millis(500));
                return refuse(stream, "wrong agent token");
            }
            let info = hello.runner.unwrap_or_default();
            if let Some(why) = mismatch(&info) {
                return refuse(stream, &why);
            }
            agent_connected(stream, info, inner)
        }
        Ok(v1::Role::Robot) => {
            let mut state = inner.state.lock().unwrap();
            let found = state.expected.iter().fold(None, |found, (i, t)| {
                let matches = same_secret(hello.token.as_bytes(), t.as_bytes());
                found.or(matches.then_some(*i))
            });
            let Some(index) = found else {
                drop(state);
                return refuse(stream, "unknown robot token");
            };
            if state.taken.contains(&index) {
                drop(state);
                return refuse(stream, "robot token already used");
            }
            state.taken.push(index);
            match state.tx.clone() {
                Some(tx) => {
                    if let Some(live) = connect_robot(index, stream, &tx) {
                        state.live.push((index, live));
                    }
                }
                None => state.queued.push((index, stream)),
            }
            Ok(())
        }
        _ => refuse(stream, "only runner agents and their robots connect here"),
    }
}

/// Why an agent can't run this server's robots: another engine version, or a robot host
/// with other deterministic rules (they would fight other battles).
fn mismatch(info: &v1::RunnerInfo) -> Option<String> {
    let engine = robocode_s_engine::ENGINE_VERSION;
    if info.engine_version != engine {
        return Some(format!(
            "the agent has engine version {}, this server {engine}: run the same robocode-s \
             release on both",
            info.engine_version
        ));
    }
    let rules = crate::DETERMINISTIC_RULES;
    if info.deterministic_rules != rules {
        return Some(format!(
            "the agent's robot host follows deterministic rules {}, this server {rules}: \
             rebuild or update it",
            info.deterministic_rules
        ));
    }
    None
}

fn agent_connected(mut stream: Stream, info: v1::RunnerInfo, inner: &Arc<Inner>) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let ack = v1::HelloAck {
        version: Some(PROTOCOL_VERSION),
        ..Default::default()
    };
    write_message(&mut stream, &server(server_message::Msg::HelloAck(ack)))?;
    let mut reader = reader_of(&stream)?;
    let peer = stream.peer_addr()?;
    let (tx, events) = mpsc::channel();
    let shared = inner.clone();
    thread::spawn(move || {
        let why = loop {
            match read_message::<v1::ClientMessage>(&mut reader) {
                Ok(Some(v1::ClientMessage { msg: Some(m) })) => {
                    let _ = tx.send(AgentEvent::Message(Box::new(m)));
                }
                Ok(Some(_)) => {}
                Ok(None) => break "its connection was closed".to_string(),
                Err(e) => break format!("reading from it failed: {e}"),
            }
        };
        // One that leaves while it waits for work isn't taken anymore.
        shared.state.lock().unwrap().idle.retain(|a| a.id != id);
        let _ = tx.send(AgentEvent::Gone(why));
    });
    let agent = Agent {
        id,
        info,
        peer,
        stream,
        events,
    };
    inner.state.lock().unwrap().idle.push(agent);
    inner.changed.notify_all();
    Ok(())
}
