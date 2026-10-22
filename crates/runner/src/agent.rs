// SPDX-License-Identifier: EPL-2.0

//! A runner agent (`robocode-s agent`, ADR 0020): connects to a battle server with the
//! agent token, waits for robots to start, fetches their jars (cached by hash), starts
//! them in its sandbox with stdio, and relays each robot's stdio over a connection of
//! its own to the server, as that robot. After a battle, or when the connection drops,
//! it connects again. The server's side is [`crate::remote`].

use crate::pool::{Pool, launch_with};
use crate::remote::{MAX_JAR_SIZE, sha256_hex};
use crate::{JavaHost, RobotLaunch, RobotProcess, RobotStdin, RobotStdout, Transport};
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::net::{self, Stream};
use robocode_s_server::tls::Fingerprint;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// How long connecting, the TLS handshake and the server's greeting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest wait between attempts to connect.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The heap of the robots of a battle run with `robocode-s battle`'s defaults: what the
/// agent warms hosts for before it has seen a battle.
const USUAL_MEMORY_MB: u32 = 512;

/// Where and how the agent connects.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The battle's `--agents` address, `host:port`.
    pub server: String,
    pub token: String,
    /// The server certificate's fingerprint; `None`: plain on loopback, else a CA-issued
    /// certificate.
    pub pin: Option<Fingerprint>,
    /// Robots at once; 0: no limit.
    pub slots: u32,
    /// Its jar cache (`jars/<sha256>/<name>`) and robot homes (`battles`).
    pub dir: PathBuf,
}

/// Starts robots for the agent.
pub trait Launcher: Send + Sync {
    /// What its robots run on, and the deterministic rules its robot host follows.
    fn info(&self) -> (String, u32);
    fn launch(&self, launch: &RobotLaunch) -> io::Result<Box<dyn Robot>>;
    /// A battle ended: the time to prepare for the next one.
    fn battle_over(&self) {}
}

/// A robot the agent started.
pub trait Robot: Send {
    fn take_pipes(&mut self) -> Option<(RobotStdout, RobotStdin)>;
    fn kill(&mut self);
    /// Its exit code once it ended.
    fn exit_code(&mut self) -> Option<i32>;
    fn stderr_tail(&self) -> String;
}

impl Robot for RobotProcess {
    fn take_pipes(&mut self) -> Option<(RobotStdout, RobotStdin)> {
        RobotProcess::take_pipes(self)
    }

    fn kill(&mut self) {
        let _ = RobotProcess::kill(self);
        let _ = self.wait();
    }

    fn exit_code(&mut self) -> Option<i32> {
        self.try_wait()
            .ok()
            .flatten()
            .map(|s| s.code().unwrap_or(-1))
    }

    fn stderr_tail(&self) -> String {
        RobotProcess::stderr_tail(self)
    }
}

/// Robot JVMs in the host's sandbox.
pub struct JvmLauncher {
    host: JavaHost,
    platform: String,
    rules: u32,
    /// Robot hosts started ahead, how many to keep, and the heap and mode of the last launch.
    pool: Option<Warm>,
}

struct Warm {
    pool: Pool,
    count: usize,
    last: Mutex<(u32, bool)>,
}

impl JvmLauncher {
    /// Asks the robot host what it runs on and which rules it follows (`dir` for its
    /// listing's home), and starts `warm` robot hosts ahead (0: none): for the usual
    /// battle, and again after each battle. The server decides the heap and the mode at
    /// each launch, and hosts that don't fit are replaced.
    pub fn new(host: JavaHost, dir: &Path, warm: usize) -> Result<Self, String> {
        let (jvm, rules) = crate::local::host_info(&host, &dir.join("describe"))?;
        let platform = format!(
            "{jvm}, {} {}, sandbox {}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            host.sandbox.report()
        );
        let pool = (warm > 0).then(|| {
            let pool = Pool::new(dir.join("pool"));
            pool.warm(&host, USUAL_MEMORY_MB, true, warm);
            Warm {
                pool,
                count: warm,
                last: Mutex::new((USUAL_MEMORY_MB, true)),
            }
        });
        Ok(Self {
            host,
            platform,
            rules,
            pool,
        })
    }
}

impl Launcher for JvmLauncher {
    fn info(&self) -> (String, u32) {
        (self.platform.clone(), self.rules)
    }

    fn launch(&self, launch: &RobotLaunch) -> io::Result<Box<dyn Robot>> {
        let pool = self.pool.as_ref();
        if let Some(warm) = pool {
            *warm.last.lock().unwrap() = (launch.memory_mb, launch.deterministic);
        }
        Ok(Box::new(launch_with(
            pool.map(|w| &w.pool),
            &self.host,
            launch,
        )?))
    }

    fn battle_over(&self) {
        if let Some(warm) = &self.pool {
            let (memory_mb, deterministic) = *warm.last.lock().unwrap();
            warm.pool
                .warm(&self.host, memory_mb, deterministic, warm.count);
        }
    }
}

/// Serves battles until `stop` is set or the server refuses the agent. `status` hears
/// what it does (connected, robots started, ...).
pub fn run(
    config: &AgentConfig,
    launcher: &dyn Launcher,
    stop: &AtomicBool,
    status: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let mut backoff = Duration::from_secs(1);
    let mut said = String::new();
    while !stop.load(Ordering::SeqCst) {
        match session(config, launcher, stop, status) {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(Ended::Refused(why)) => return Err(why),
            Err(Ended::Retry(why)) => {
                // The same failure again (no battle server yet) is said once.
                if why != said {
                    status(&format!("{why}; trying again"));
                    said = why;
                }
                let mut waited = Duration::ZERO;
                while waited < backoff && !stop.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(100));
                    waited += Duration::from_millis(100);
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    Ok(())
}

/// Why a session ended early.
#[derive(Debug)]
pub enum Ended {
    /// The server refused the agent (token, versions) or its certificate: no use trying
    /// again.
    Refused(String),
    /// No server, or the connection broke.
    Retry(String),
}

/// One connection to the server: one battle's robots, or none.
pub fn session(
    config: &AgentConfig,
    launcher: &dyn Launcher,
    stop: &AtomicBool,
    status: &mut dyn FnMut(&str),
) -> Result<(), Ended> {
    let mut stream = net::connect(&config.server, config.pin, CONNECT_TIMEOUT).map_err(|e| {
        if e.contains("TLS") {
            Ended::Refused(e)
        } else {
            Ended::Retry(e)
        }
    })?;
    let (platform, rules) = launcher.info();
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Runner as i32,
            token: config.token.clone(),
            runner: Some(v1::RunnerInfo {
                runtimes: vec!["jvm".into()],
                mode: "remote".into(),
                trusted_cpu_time: true,
                engine_version: robocode_s_engine::ENGINE_VERSION,
                deterministic_rules: rules,
                platform,
                slots: config.slots,
            }),
            ..Default::default()
        })),
    };
    let broke = |e: io::Error| Ended::Retry(format!("{}: {e}", config.server));
    stream
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(broke)?;
    write_message(&mut stream, &hello).map_err(broke)?;
    match read_message::<v1::ServerMessage>(&mut stream)
        .map_err(broke)?
        .and_then(|m| m.msg)
    {
        Some(server_message::Msg::HelloAck(_)) => {}
        Some(server_message::Msg::Error(e)) => {
            return Err(Ended::Refused(format!(
                "{} refused the agent: {}",
                config.server, e.message
            )));
        }
        _ => {
            return Err(Ended::Refused(format!(
                "{}: not a robocode-s battle's agent listener",
                config.server
            )));
        }
    }
    stream.set_read_timeout(None).map_err(broke)?;
    status(&format!(
        "connected to {}, waiting for robots",
        config.server
    ));
    let mut battle = Battle {
        config,
        launcher,
        inbox: Inbox {
            stream,
            deferred: VecDeque::new(),
        },
        robots: HashMap::new(),
        home: config.dir.join("battles").join(format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis()),
            std::process::id()
        )),
        started: 0,
    };
    let result = battle.serve(stop, status);
    for robot in battle.robots.values_mut() {
        robot.kill();
    }
    let _ = std::fs::remove_dir_all(&battle.home);
    result
}

/// What the server sends, with what came while the agent waited for a jar.
struct Inbox {
    stream: Stream,
    deferred: VecDeque<server_message::Msg>,
}

impl Inbox {
    fn next(&mut self) -> io::Result<Option<server_message::Msg>> {
        if let Some(m) = self.deferred.pop_front() {
            return Ok(Some(m));
        }
        loop {
            match read_message::<v1::ServerMessage>(&mut self.stream)? {
                Some(v1::ServerMessage { msg: Some(m) }) => return Ok(Some(m)),
                Some(_) => {}
                None => return Ok(None),
            }
        }
    }

    /// The next part of a jar; other messages wait for [`Inbox::next`].
    fn jar_data(&mut self) -> io::Result<v1::JarData> {
        loop {
            match read_message::<v1::ServerMessage>(&mut self.stream)? {
                Some(v1::ServerMessage {
                    msg: Some(server_message::Msg::JarData(d)),
                }) => return Ok(d),
                Some(v1::ServerMessage {
                    msg: Some(server_message::Msg::Error(e)),
                }) => return Err(io::Error::other(e.message)),
                Some(v1::ServerMessage { msg: Some(m) }) => self.deferred.push_back(m),
                Some(_) => {}
                None => return Err(io::ErrorKind::UnexpectedEof.into()),
            }
        }
    }

    fn send(&mut self, msg: client_message::Msg) -> io::Result<()> {
        write_message(&mut self.stream, &v1::ClientMessage { msg: Some(msg) })
    }
}

struct Battle<'a> {
    config: &'a AgentConfig,
    launcher: &'a dyn Launcher,
    inbox: Inbox,
    robots: HashMap<String, Box<dyn Robot>>,
    home: PathBuf,
    started: usize,
}

impl Battle<'_> {
    fn serve(&mut self, stop: &AtomicBool, status: &mut dyn FnMut(&str)) -> Result<(), Ended> {
        // A stop ends the connection, and with it the wait for the next message.
        let handle = self.inbox.stream.try_clone().ok();
        let done = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::SeqCst) {
                    if stop.load(Ordering::SeqCst) {
                        if let Some(h) = &handle {
                            let _ = h.shutdown();
                        }
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            });
            let result = self.messages(status);
            done.store(true, Ordering::SeqCst);
            result
        })
    }

    fn messages(&mut self, status: &mut dyn FnMut(&str)) -> Result<(), Ended> {
        let server = self.config.server.clone();
        let broke = |e: io::Error| Ended::Retry(format!("{server}: {e}"));
        loop {
            let message = match self.inbox.next() {
                Ok(Some(m)) => m,
                Ok(None) => {
                    return if self.started > 0 {
                        Ok(())
                    } else {
                        Err(Ended::Retry(format!("{server} ended the connection")))
                    };
                }
                Err(e) => return Err(broke(e)),
            };
            match message {
                server_message::Msg::LaunchRobot(launch) => {
                    let id = launch.robot_id.clone();
                    let name = launch.robot_name.clone();
                    // Its relay answers `LaunchResult` once the robot is connected.
                    let error = match self.launch(launch, status) {
                        Ok(()) => {
                            self.started += 1;
                            status(&format!("started {name}"));
                            continue;
                        }
                        Err(e) => e,
                    };
                    status(&format!("could not start {name}: {error}"));
                    let failed = v1::LaunchResult {
                        robot_id: id,
                        ok: false,
                        error,
                    };
                    self.inbox
                        .send(client_message::Msg::LaunchResult(failed))
                        .map_err(broke)?;
                }
                server_message::Msg::StopRobot(stop) => {
                    let Some(mut robot) = self.robots.remove(&stop.robot_id) else {
                        continue;
                    };
                    // It ends by itself when the battle closes its connection.
                    for _ in 0..40 {
                        if robot.exit_code().is_some() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                    robot.kill();
                    let exited = v1::RobotExited {
                        robot_id: stop.robot_id,
                        exit_code: robot.exit_code().unwrap_or(-1),
                        stderr_tail: robot.stderr_tail(),
                    };
                    self.inbox
                        .send(client_message::Msg::RobotExited(exited))
                        .map_err(broke)?;
                    if self.robots.is_empty() {
                        status("battle over");
                        self.launcher.battle_over();
                        return Ok(());
                    }
                }
                server_message::Msg::Error(e) => {
                    return Err(Ended::Retry(format!("{server}: {}", e.message)));
                }
                _ => {}
            }
        }
    }

    /// Fetches the jars, starts the robot and connects it: its relay answers
    /// `LaunchResult` once the robot said `Hello` to the server.
    fn launch(
        &mut self,
        launch: v1::LaunchRobot,
        status: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let index: usize = launch
            .robot_id
            .parse()
            .map_err(|_| format!("bad robot id {}", launch.robot_id))?;
        if self.config.slots > 0 && self.robots.len() >= self.config.slots as usize {
            return Err(format!("no free slot (--slots {})", self.config.slots));
        }
        let mut jars = Vec::new();
        for jar in &launch.jars {
            jars.push(self.jar(jar, status)?);
        }
        let spec = RobotLaunch {
            transport: Transport::Stdio,
            robot: launch.lookup_name.clone(),
            jars,
            home: self.home.join(format!("robot-{index}")),
            seed: launch.battle_seed,
            deterministic: launch.deterministic,
            memory_mb: launch.memory_limit_mb.max(64),
        };
        let mut robot = self
            .launcher
            .launch(&spec)
            .map_err(|e| format!("launching: {e}"))?;
        let (stdout, stdin) = robot
            .take_pipes()
            .ok_or("the robot has no pipes".to_string())?;
        let results = self.inbox.stream.try_clone().map_err(|e| e.to_string())?;
        let relay = Relay {
            server: self.config.server.clone(),
            pin: self.config.pin,
            token: launch.session_token,
            robot_id: launch.robot_id.clone(),
        };
        thread::spawn(move || relay.run(stdout, stdin, results));
        self.robots.insert(launch.robot_id, robot);
        Ok(())
    }

    /// The cached jar, else fetched from the server.
    fn jar(&mut self, jar: &v1::JarRef, status: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
        let cache = self.config.dir.join("jars");
        if let Some(path) = cached_jar(&cache, jar)? {
            return Ok(path);
        }
        let request = v1::JarRequest {
            sha256: jar.sha256.clone(),
        };
        self.inbox
            .send(client_message::Msg::JarRequest(request))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::with_capacity(jar.size as usize);
        loop {
            let part = self.inbox.jar_data().map_err(|e| e.to_string())?;
            if part.sha256 != jar.sha256 || part.offset != bytes.len() as u64 {
                return Err(format!("{}: parts out of order", jar.name));
            }
            if bytes.len() as u64 + part.data.len() as u64 > jar.size {
                return Err(format!("{}: larger than said", jar.name));
            }
            bytes.extend_from_slice(&part.data);
            if part.last {
                break;
            }
        }
        let path = store_jar(&cache, jar, &bytes)?;
        status(&format!("fetched {} ({} bytes)", jar.name, jar.size));
        Ok(path)
    }
}

/// Where the jar `jar` is kept in `cache`: `<sha256>/<name>`, the name without folders.
pub fn jar_path(cache: &Path, jar: &v1::JarRef) -> Result<PathBuf, String> {
    let hex = jar.sha256.as_str();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("not a SHA-256: {hex}"));
    }
    let name = Path::new(&jar.name)
        .file_name()
        .filter(|n| !n.is_empty())
        .map_or_else(|| "robot.jar".into(), |n| n.to_owned());
    Ok(cache.join(hex.to_ascii_lowercase()).join(name))
}

/// The cached copy of `jar`, if one with its size and hash is there.
pub fn cached_jar(cache: &Path, jar: &v1::JarRef) -> Result<Option<PathBuf>, String> {
    if jar.size > MAX_JAR_SIZE {
        return Err(format!("{}: larger than {MAX_JAR_SIZE} bytes", jar.name));
    }
    let path = jar_path(cache, jar)?;
    match std::fs::read(&path) {
        Ok(bytes) if bytes.len() as u64 == jar.size && sha256_hex(&bytes) == jar.sha256 => {
            Ok(Some(path))
        }
        _ => Ok(None),
    }
}

/// Keeps `bytes` as `jar` in `cache` once they have its hash.
pub fn store_jar(cache: &Path, jar: &v1::JarRef, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = jar_path(cache, jar)?;
    if bytes.len() as u64 != jar.size || sha256_hex(bytes) != jar.sha256.to_ascii_lowercase() {
        return Err(format!("{}: the bytes sent don't have its hash", jar.name));
    }
    let dir = path.parent().unwrap_or(cache);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Relays one robot's stdio over a connection of its own, as that robot.
struct Relay {
    server: String,
    pin: Option<Fingerprint>,
    token: String,
    robot_id: String,
}

impl Relay {
    fn run(self, mut stdout: RobotStdout, mut stdin: RobotStdin, mut results: Stream) {
        let result = self.connect(&mut stdout);
        let launched = v1::LaunchResult {
            robot_id: self.robot_id.clone(),
            ok: result.is_ok(),
            error: result.as_ref().err().cloned().unwrap_or_default(),
        };
        let message = v1::ClientMessage {
            msg: Some(client_message::Msg::LaunchResult(launched)),
        };
        let _ = write_message(&mut results, &message);
        let Ok(connection) = result else {
            return;
        };
        let Ok(mut up) = connection.try_clone() else {
            return;
        };
        let mut down = connection;
        // The robot's messages to the server; at their end, the connection's.
        thread::spawn(move || {
            let _ = copy(&mut stdout, &mut up);
            let _ = up.shutdown();
        });
        // The server's to the robot; at their end the robot's stdin closes: it ends.
        let _ = copy(&mut down, &mut stdin);
    }

    /// Takes the robot's `Hello`, connects to the server and says it with the robot's
    /// token.
    fn connect(&self, stdout: &mut RobotStdout) -> Result<Stream, String> {
        let mut hello = match read_message::<v1::ClientMessage>(stdout) {
            Ok(Some(v1::ClientMessage {
                msg: Some(client_message::Msg::Hello(h)),
            })) => h,
            Ok(_) => return Err("the robot didn't say Hello".into()),
            Err(e) => return Err(format!("reading the robot's Hello: {e}")),
        };
        hello.token = self.token.clone();
        let mut connection = net::connect(&self.server, self.pin, CONNECT_TIMEOUT)?;
        let message = v1::ClientMessage {
            msg: Some(client_message::Msg::Hello(hello)),
        };
        write_message(&mut connection, &message).map_err(|e| e.to_string())?;
        Ok(connection)
    }
}

/// Copies until the end of `from`, flushing each write: the battle is in lockstep.
fn copy(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = match from.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buffer[..n])?;
        to.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jars_are_cached_by_hash_and_checked() {
        let cache = std::env::temp_dir().join(format!("robocodes-jars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let bytes = b"PK fake jar".to_vec();
        let jar = v1::JarRef {
            name: "../../evil/robots.jar".into(),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        };
        assert_eq!(cached_jar(&cache, &jar).unwrap(), None);
        let wrong = v1::JarRef {
            sha256: sha256_hex(b"other"),
            ..jar.clone()
        };
        assert!(store_jar(&cache, &wrong, &bytes).is_err(), "not its hash");
        let path = store_jar(&cache, &jar, &bytes).unwrap();
        assert_eq!(
            path,
            cache.join(&jar.sha256).join("robots.jar"),
            "no folders"
        );
        assert_eq!(cached_jar(&cache, &jar).unwrap(), Some(path.clone()));
        // A changed copy isn't used.
        std::fs::write(&path, b"PK fake jaR").unwrap();
        assert_eq!(cached_jar(&cache, &jar).unwrap(), None);
        let bad = v1::JarRef {
            sha256: "../x".into(),
            ..jar
        };
        assert!(jar_path(&cache, &bad).is_err());
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
