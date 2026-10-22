// SPDX-License-Identifier: EPL-2.0

//! Battles with robots on runner agents (ADR 0020): this machine prepares the battle (it
//! has the jars, and describes the robots in its own sandbox) and runs the server; the
//! agents start the robots and relay their stdio. The agent's side is [`crate::agent`].

use crate::local::{LocalBattle, LocalOutcome, serve, without_noise};
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_server::agents::{Agent, AgentEvent, AgentListener};
use robocode_s_server::{Observers, random_token};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long agents get to fetch the jars and start every robot.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(180);

/// How long agents get to stop their robots and send their error output.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// A jar is sent in parts of this size (messages are at most 16 MiB).
pub const JAR_CHUNK: usize = 1 << 20;

/// Jars larger than this aren't sent or taken.
pub const MAX_JAR_SIZE: u64 = 256 << 20;

/// The end of a battle on agents.
#[derive(Debug)]
pub struct RemoteOutcome {
    pub local: LocalOutcome,
    /// Per robot, the agent it ran on (address and platform).
    pub agents: Vec<String>,
}

/// A jar of the battle, by content.
#[derive(Debug, Clone)]
struct Jar {
    reference: v1::JarRef,
    path: PathBuf,
}

/// The SHA-256 of `bytes`, hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn jars(battle: &LocalBattle) -> Result<Vec<Jar>, String> {
    battle
        .jars
        .iter()
        .map(|path| {
            if path.is_dir() {
                return Err(format!(
                    "{}: a class folder can't go to runner agents; build a jar of it",
                    path.display()
                ));
            }
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if bytes.len() as u64 > MAX_JAR_SIZE {
                return Err(format!(
                    "{}: larger than {MAX_JAR_SIZE} bytes",
                    path.display()
                ));
            }
            let name = path
                .file_name()
                .map_or("robot.jar".into(), |n| n.to_string_lossy().into_owned());
            Ok(Jar {
                reference: v1::JarRef {
                    name,
                    sha256: sha256_hex(&bytes),
                    size: bytes.len() as u64,
                },
                path: path.clone(),
            })
        })
        .collect()
}

/// Which agent runs each robot: round robin over the agents with a free slot.
fn assign(robots: usize, slots: &[u32]) -> Result<Vec<usize>, String> {
    let mut used = vec![0u32; slots.len()];
    let mut next = 0;
    (0..robots)
        .map(|_| {
            for k in 0..slots.len() {
                let a = (next + k) % slots.len();
                if slots[a] == 0 || used[a] < slots[a] {
                    used[a] += 1;
                    next = a + 1;
                    return Ok(a);
                }
            }
            Err(format!(
                "{robots} robots, but the runner agents have {} slots",
                slots.iter().sum::<u32>()
            ))
        })
        .collect()
}

/// Fights `battle` with its robots on the agents at `agents`: waits up to `wait_timeout`
/// for at least `wait` of them, has them start the robots, then serves the battle as
/// [`crate::local::run_local_battle_with`] does.
pub fn run_remote_battle(
    battle: &LocalBattle,
    agents: &AgentListener,
    (wait, wait_timeout): (usize, Duration),
    observers: Option<Observers>,
    controls: Option<mpsc::Receiver<v1::ObserverControl>>,
    observe: impl FnMut(&server_message::Msg),
) -> Result<RemoteOutcome, String> {
    let jars = jars(battle)?;
    let taken = agents.take_agents(wait, wait_timeout).map_err(|n| {
        format!(
            "{n} of {wait} runner agents connected within {} s",
            wait_timeout.as_secs()
        )
    })?;
    let slots: Vec<u32> = taken.iter().map(|a| a.info.slots).collect();
    let placement = assign(battle.robots.len(), &slots)?;
    let described: Vec<String> = taken
        .iter()
        .map(|a| format!("{} ({})", a.peer, a.info.platform))
        .collect();
    let tokens: Vec<String> = battle.robots.iter().map(|_| random_token()).collect();
    agents.expect(tokens.iter().cloned().enumerate().collect());

    let stop = Arc::new(AtomicBool::new(false));
    let tails = Arc::new(Mutex::new(vec![String::new(); battle.robots.len()]));
    let (launched_tx, launched) = mpsc::channel();
    let sessions: Vec<_> = taken
        .into_iter()
        .enumerate()
        .map(|(a, agent)| {
            let robots: Vec<usize> = (0..placement.len())
                .filter(|&i| placement[i] == a)
                .collect();
            let launches = robots
                .iter()
                .map(|&i| {
                    let robot = &battle.robots[i];
                    v1::LaunchRobot {
                        robot_id: i.to_string(),
                        session_token: tokens[i].clone(),
                        robot_name: robot.statics.name.clone(),
                        lookup_name: robot.lookup.clone(),
                        battle_seed: battle.config.seed,
                        deterministic: battle.config.deterministic,
                        memory_limit_mb: battle.memory_mb,
                        jars: jars.iter().map(|j| j.reference.clone()).collect(),
                        ..Default::default()
                    }
                })
                .collect();
            let session = Session {
                agent,
                launches,
                jars: jars.clone(),
                listener: agents.clone(),
                stop: stop.clone(),
                tails: tails.clone(),
                launched: launched_tx.clone(),
            };
            thread::spawn(move || session.run())
        })
        .collect();
    drop(launched_tx);

    let finish = |sessions: Vec<thread::JoinHandle<()>>| {
        stop.store(true, Ordering::SeqCst);
        for s in sessions {
            let _ = s.join();
        }
    };
    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    let mut pending = battle.robots.len();
    while pending > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        match launched.recv_timeout(left) {
            Ok((_, Ok(()))) => pending -= 1,
            Ok((i, Err(e))) => {
                finish(sessions);
                return Err(format!(
                    "{} on {}: {e}",
                    battle.robots[i].statics.name, described[placement[i]]
                ));
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                finish(sessions);
                return Err(format!(
                    "the runner agents didn't start every robot within {} s",
                    LAUNCH_TIMEOUT.as_secs()
                ));
            }
        }
    }
    let outcome = serve(
        battle,
        Vec::new(),
        observers,
        Some(agents.clone()),
        controls,
        observe,
    );
    finish(sessions);
    let outcome = outcome?;
    let stderr = tails
        .lock()
        .unwrap()
        .iter()
        .map(|t| without_noise(t))
        .collect();
    Ok(RemoteOutcome {
        local: LocalOutcome { outcome, stderr },
        agents: placement.iter().map(|&a| described[a].clone()).collect(),
    })
}

/// One agent's part of the battle, on a thread of its own: it starts its robots, sends
/// their jars, sees it leave (its robots then leave too), and stops them at the end.
struct Session {
    agent: Agent,
    launches: Vec<v1::LaunchRobot>,
    jars: Vec<Jar>,
    listener: AgentListener,
    stop: Arc<AtomicBool>,
    tails: Arc<Mutex<Vec<String>>>,
    launched: mpsc::Sender<(usize, Result<(), String>)>,
}

impl Session {
    fn run(mut self) {
        let robots: Vec<usize> = self
            .launches
            .iter()
            .filter_map(|l| l.robot_id.parse().ok())
            .collect();
        for launch in std::mem::take(&mut self.launches) {
            if let Err(e) = self.agent.send(server_message::Msg::LaunchRobot(launch)) {
                return self.gone(&robots, &format!("sending to it failed: {e}"));
            }
        }
        let mut waiting: Vec<usize> = robots.clone();
        let mut stopping: Option<Instant> = None;
        loop {
            if stopping.is_none() && self.stop.load(Ordering::SeqCst) {
                for &i in &robots {
                    let stop = v1::StopRobot {
                        robot_id: i.to_string(),
                    };
                    let _ = self.agent.send(server_message::Msg::StopRobot(stop));
                }
                stopping = Some(Instant::now() + STOP_TIMEOUT);
            }
            if stopping.is_some_and(|s| Instant::now() > s || waiting.is_empty()) {
                break;
            }
            let event = match self.agent.events.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let event = match event {
                AgentEvent::Message(m) => Ok(*m),
                AgentEvent::Gone(why) => Err(why),
            };
            match event {
                Ok(client_message::Msg::JarRequest(request)) => {
                    if let Err(e) = self.send_jar(&request.sha256) {
                        robocode_s_server::log_perf(format_args!("agent {e}"));
                        let error = v1::Error { message: e };
                        let _ = self.agent.send(server_message::Msg::Error(error));
                        self.agent.shutdown();
                    }
                }
                Ok(client_message::Msg::LaunchResult(r)) => {
                    if let Ok(i) = r.robot_id.parse::<usize>()
                        && robots.contains(&i)
                    {
                        let result = if r.ok { Ok(()) } else { Err(r.error) };
                        let _ = self.launched.send((i, result));
                    }
                }
                Ok(client_message::Msg::RobotExited(exited)) => {
                    if let Ok(i) = exited.robot_id.parse::<usize>()
                        && robots.contains(&i)
                    {
                        self.tails.lock().unwrap()[i] = exited.stderr_tail;
                        waiting.retain(|&w| w != i);
                    }
                }
                Ok(_) => {}
                Err(why) => return self.gone(&robots, &why),
            }
        }
        self.agent.shutdown();
    }

    /// The agent left: its robots' connections end, so the battle goes on without them
    /// (as with robots that crashed); those not started yet won't be.
    fn gone(&self, robots: &[usize], why: &str) {
        for &i in robots {
            self.listener.disconnect(i);
            let _ = self
                .launched
                .send((i, Err(format!("the runner agent left: {why}"))));
            let mut tails = self.tails.lock().unwrap();
            if tails[i].is_empty() {
                tails[i] = format!("its runner agent left: {why}");
            }
        }
    }

    /// Sends the jar `sha256` in parts: only one of the battle's.
    fn send_jar(&mut self, sha256: &str) -> Result<(), String> {
        let jar = self
            .jars
            .iter()
            .find(|j| j.reference.sha256 == sha256)
            .ok_or_else(|| format!("asked for a jar that isn't the battle's: {sha256}"))?;
        let mut file =
            std::fs::File::open(&jar.path).map_err(|e| format!("{}: {e}", jar.path.display()))?;
        let mut offset = 0u64;
        loop {
            let mut data = Vec::with_capacity(JAR_CHUNK);
            (&mut file)
                .take(JAR_CHUNK as u64)
                .read_to_end(&mut data)
                .map_err(|e| format!("{}: {e}", jar.path.display()))?;
            let n = data.len() as u64;
            let last = offset + n >= jar.reference.size;
            let part = v1::JarData {
                sha256: sha256.to_string(),
                offset,
                data,
                last,
            };
            self.agent
                .send(server_message::Msg::JarData(part))
                .map_err(|e| e.to_string())?;
            offset += n;
            if last {
                return Ok(());
            }
            if n == 0 {
                return Err(format!("{}: shorter than before", jar.path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_go_round_robin_to_free_slots() {
        assert_eq!(assign(5, &[0, 0]).unwrap(), [0, 1, 0, 1, 0]);
        assert_eq!(assign(4, &[1, 0, 2]).unwrap(), [0, 1, 2, 1]);
        assert_eq!(assign(3, &[1, 2]).unwrap(), [0, 1, 1]);
        assert!(assign(4, &[1, 2]).unwrap_err().contains("3 slots"));
    }

    #[test]
    fn jars_are_named_by_their_hash() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
