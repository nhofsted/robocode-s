// SPDX-License-Identifier: EPL-2.0

//! Robots started by an IDE (#17, design §5.2): the IDE's debugger runs the robot host, which
//! connects here first, describes the robot in its `Hello` and waits; the battle is made
//! later, from the robots that are here ([`IdeListener::take`]). It listens on loopback only,
//! with a token that is kept in the data folder ([`token`]) so that the IDE's run
//! configuration never changes and no other user can take the slot.
//!
//! A robot taken for a battle becomes a [`RobotPipe`]: the battle loop reads its `Hello`
//! again (from the bytes kept here) and goes on as for a robot on pipes.

use crate::net::{self, Stream};
use crate::{HELLO_TIMEOUT, RobotPipe, read_hello, refuse, same_secret};
use robocode_s_protocol::v1::{self, client_message};
use robocode_s_protocol::{PROTOCOL_VERSION, write_message};
use std::io::{self, Cursor, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// The port robots from an IDE connect to unless chosen otherwise.
pub const DEFAULT_PORT: u16 = 7878;

/// The file in the data folder holding the token.
pub const TOKEN_FILE: &str = "ide-token";

/// At most this many robots wait at once, and this many handshakes (up to the token check).
const MAX_ROBOTS: usize = 16;
const MAX_HANDSHAKES: usize = 16;

/// The IDE token in the data folder `data`, made the first time (128 random bits as hex,
/// readable by the user only).
pub fn token(data: &Path) -> Result<String, String> {
    let path = data.join(TOKEN_FILE);
    let error = |e: io::Error| format!("{}: {e}", path.display());
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim();
        if !text.is_empty() {
            return Ok(text.to_string());
        }
    }
    std::fs::create_dir_all(data).map_err(error)?;
    let token = crate::random_token();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .and_then(|mut f| f.write_all(token.as_bytes()))
        .map_err(error)?;
    Ok(token)
}

/// What a robot from an IDE says about itself.
#[derive(Debug, Clone)]
pub struct IdeRobot {
    /// Names the robot while it waits (for [`IdeListener::take`]).
    pub id: u64,
    pub description: v1::RobotDescription,
    pub peer: SocketAddr,
}

/// Why a robot's description is not accepted, if it isn't.
pub fn check_description(d: &v1::RobotDescription) -> Result<(), &'static str> {
    if d.name.is_empty() || d.full_class_name.is_empty() {
        return Err("the description has no robot name");
    }
    if d.team_id.is_some() || d.r#type.is_some_and(|t| t.team) {
        return Err("teams can't be started from an IDE (a team needs several JVMs)");
    }
    Ok(())
}

/// The listener for robots from an IDE. Clones share it.
#[derive(Clone)]
pub struct IdeListener {
    inner: Arc<Inner>,
}

struct Inner {
    address: SocketAddr,
    stop: AtomicBool,
    state: Mutex<Vec<Held>>,
    changed: Condvar,
}

/// A robot that has said `Hello` and waits for its battle.
struct Held {
    robot: IdeRobot,
    /// Its `Hello`, which the battle reads again.
    hello: v1::Hello,
    stream: Stream,
    /// What the robot sends, read by a thread so that its leaving is seen at once.
    bytes: Receiver<Vec<u8>>,
}

impl IdeListener {
    /// Listens on `address` (loopback) for robots presenting `token`.
    pub fn bind(address: &str, token: String) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        net::check_listener(&listener, None)?;
        let inner = Arc::new(Inner {
            address: listener.local_addr()?,
            stop: AtomicBool::new(false),
            state: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        });
        listener.set_nonblocking(true)?;
        let shared = inner.clone();
        thread::spawn(move || accept_loop(&listener, &shared, &token));
        Ok(Self { inner })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.address
    }

    /// Stops listening and sends the waiting robots away.
    pub fn close(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        for held in self.inner.state.lock().unwrap().drain(..) {
            let _ = held.stream.shutdown();
        }
        self.inner.changed.notify_all();
    }

    /// The robots waiting now, the oldest first.
    pub fn robots(&self) -> Vec<IdeRobot> {
        let state = self.inner.state.lock().unwrap();
        state.iter().map(|h| h.robot.clone()).collect()
    }

    /// Waits until `count` robots are here or `cancel` is set (checked every 100 ms);
    /// `count` robots, the oldest first, or `None` when cancelled.
    pub fn wait_for(&self, count: usize, cancel: &AtomicBool) -> Option<Vec<IdeRobot>> {
        let mut state = self.inner.state.lock().unwrap();
        while state.len() < count {
            if cancel.load(Ordering::SeqCst) {
                return None;
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap()
                .0;
        }
        Some(state.iter().map(|h| h.robot.clone()).collect())
    }

    /// Waits until something changed in the robots here (one coming or going) or `wait`
    /// passed.
    pub fn wait_change(&self, wait: Duration) {
        let state = self.inner.state.lock().unwrap();
        let _ = self.inner.changed.wait_timeout(state, wait).unwrap();
    }

    /// Takes robot `id` for a battle, as robot `index` of it; the battle loop sees its
    /// `Hello` and waits for it without time limits (`RobotEntry::debug`). `Err` if it
    /// isn't here anymore.
    pub fn take(&self, id: u64, index: usize) -> Result<RobotPipe, String> {
        let mut state = self.inner.state.lock().unwrap();
        let at = state
            .iter()
            .position(|h| h.robot.id == id)
            .ok_or("the robot from your IDE isn't connected anymore")?;
        let held = state.remove(at);
        let mut hello = Vec::new();
        let message = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(held.hello)),
        };
        write_message(&mut hello, &message).map_err(|e| e.to_string())?;
        Ok(RobotPipe {
            robot: index,
            reader: Box::new(Cursor::new(hello).chain(ChunkReader {
                chunks: held.bytes,
                current: Cursor::new(Vec::new()),
            })),
            writer: Box::new(CloseOnDrop(held.stream)),
        })
    }
}

impl std::fmt::Debug for IdeListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IdeListener({})", self.inner.address)
    }
}

/// The bytes a reader thread passed on, as a stream; ends when that thread does.
struct ChunkReader {
    chunks: Receiver<Vec<u8>>,
    current: Cursor<Vec<u8>>,
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.current.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            match self.chunks.recv() {
                Ok(chunk) => self.current = Cursor::new(chunk),
                Err(_) => return Ok(0),
            }
        }
    }
}

/// The robot's connection as the battle's writer: ends it when the battle lets go.
struct CloseOnDrop(Stream);

impl Write for CloseOnDrop {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

fn accept_loop(listener: &TcpListener, inner: &Arc<Inner>, token: &str) {
    let handshakes = Arc::new(AtomicUsize::new(0));
    while !inner.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((socket, _)) if handshakes.load(Ordering::SeqCst) >= MAX_HANDSHAKES => {
                drop(socket);
            }
            Ok((socket, _)) => {
                let (inner, token, handshakes) =
                    (inner.clone(), token.to_string(), handshakes.clone());
                handshakes.fetch_add(1, Ordering::SeqCst);
                thread::spawn(move || {
                    if let Ok(stream) = Stream::accept(socket, None, HELLO_TIMEOUT) {
                        let _ = handshake(stream, &inner, &token);
                    }
                    handshakes.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Reads the robot's `Hello`, checks it and keeps the connection until a battle takes it.
fn handshake(mut stream: Stream, inner: &Arc<Inner>, token: &str) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let hello = match read_hello(&mut stream)? {
        Some(v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(h)),
        }) => h,
        _ => return refuse(stream, "expected Hello"),
    };
    if hello.version.map(|v| v.major) != Some(PROTOCOL_VERSION.major) {
        return refuse(stream, "unsupported protocol version");
    }
    if v1::Role::try_from(hello.role) != Ok(v1::Role::Robot) {
        return refuse(stream, "only robots from an IDE connect here");
    }
    if !same_secret(hello.token.as_bytes(), token.as_bytes()) {
        // A moment's delay makes guessing slower.
        thread::sleep(Duration::from_millis(500));
        return refuse(stream, "wrong IDE token");
    }
    let Some(description) = hello.robot_description.clone() else {
        return refuse(stream, "a robot from an IDE describes itself in its Hello");
    };
    if let Err(why) = check_description(&description) {
        return refuse(stream, why);
    }
    let mut reader = crate::reader_of(&stream)?;
    let peer = stream.peer_addr()?;
    let (tx, bytes) = mpsc::channel();
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    {
        let mut state = inner.state.lock().unwrap();
        if inner.stop.load(Ordering::SeqCst) || state.len() >= MAX_ROBOTS {
            drop(state);
            return refuse(stream, "too many robots from the IDE are waiting");
        }
        state.push(Held {
            robot: IdeRobot {
                id,
                description,
                peer,
            },
            hello,
            stream,
            bytes,
        });
    }
    inner.changed.notify_all();
    let shared = inner.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
        // Gone before its battle took it: it isn't offered anymore.
        shared.state.lock().unwrap().retain(|h| h.robot.id != id);
        shared.changed.notify_all();
    });
    Ok(())
}
