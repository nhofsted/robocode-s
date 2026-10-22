// SPDX-License-Identifier: EPL-2.0

//! Warm pools (design §6.2): robot hosts started ahead of time, each waiting on its stdin
//! for the robot it will play, so that a battle only waits for the robots' own start-up
//! (a JVM takes about 0.5 to 1 s to start, a container 0.4 s more).
//!
//! A warm host is a `RobotHost --stdio --warm` of the same sandbox, JVM settings and mode
//! as a cold one. Its sandbox grants are fixed when it starts, so it has a home of its own
//! (`robot-N` in the pool's directory) instead of grants for the robot's jars: the runner
//! copies the jars into the home's `jars` directory (not hard links: the robot may write
//! there) and sends a [`v1::RobotAssignment`]. A host plays one robot and is never
//! reused. A pool keeps hosts for one [`Key`] at a time: asking it to warm hosts for
//! another one discards the rest, so that idle JVMs don't pile up.
//!
//! The pool only helps where robots are launched again and again in one process (the
//! GUI, a rumble loop, a runner agent), or while something else is waited for (the CLI
//! describes the robots while their hosts start).

use crate::{JavaHost, RobotLaunch, RobotProcess, Sandbox, Transport, launch};
use robocode_s_protocol::v1;
use robocode_s_protocol::write_message;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What warm hosts of one pool entry have in common, and a robot's launch must match: the
/// sandbox, the Java and robot host to run, the heap, and the deterministic mode with its
/// JVM flags and agent. Everything else about a launch (robot, seed, jars, home) is the
/// assignment's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    sandbox: Sandbox,
    java: PathBuf,
    lib_dir: PathBuf,
    /// The determinism agent, in deterministic mode.
    agent_jar: Option<PathBuf>,
    memory_mb: u32,
    deterministic: bool,
}

impl Key {
    pub fn new(host: &JavaHost, memory_mb: u32, deterministic: bool) -> Self {
        let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        Self {
            sandbox: host.sandbox,
            // A bare `java` is looked up on the PATH.
            java: if host.java.components().count() > 1 {
                absolute(&host.java)
            } else {
                host.java.clone()
            },
            lib_dir: absolute(&host.lib_dir),
            agent_jar: deterministic.then(|| absolute(&host.agent_jar)),
            memory_mb,
            deterministic,
        }
    }
}

/// A started host and its home.
struct Warm {
    process: RobotProcess,
    home: PathBuf,
}

#[derive(Default)]
struct Slot {
    /// The hosts to start more of.
    host: Option<JavaHost>,
    /// Started hosts, oldest first.
    ready: Vec<Warm>,
    /// How many hosts to keep ready.
    target: usize,
    /// Hosts being started.
    starting: usize,
}

/// Robot hosts started ahead of time, in `dir`. Dropping the pool stops the hosts left.
pub struct Pool {
    dir: PathBuf,
    next: AtomicUsize,
    /// Hosts given to launches.
    assigned: AtomicUsize,
    slots: Mutex<HashMap<Key, Slot>>,
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Pool")
            .field("dir", &self.dir)
            .field("idle", &self.idle())
            .finish()
    }
}

impl Pool {
    /// A pool whose hosts' homes are `robot-N` in `dir` (made when needed).
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            next: AtomicUsize::new(0),
            assigned: AtomicUsize::new(0),
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// Makes sure `count` hosts for robots launched as `host` runs them (with a heap of
    /// `memory_mb`, `deterministic` or not) are started or starting, and drops the hosts
    /// of other kinds. Starting a host is a process spawn: it doesn't wait for the JVM.
    /// `count` is kept: [`Pool::refill`] starts that many again.
    pub fn warm(&self, host: &JavaHost, memory_mb: u32, deterministic: bool, count: usize) {
        let key = Key::new(host, memory_mb, deterministic);
        let dropped: Vec<Warm> = {
            let mut slots = self.slots.lock().unwrap();
            let other: Vec<Key> = slots.keys().filter(|k| **k != key).cloned().collect();
            let dropped = other
                .iter()
                .filter_map(|k| slots.remove(k))
                .flat_map(|s| s.ready)
                .collect();
            let slot = slots.entry(key.clone()).or_default();
            slot.host = Some(host.clone());
            slot.target = count;
            dropped
        };
        for warm in dropped {
            warm.discard();
        }
        self.top_up(&key);
    }

    /// Starts hosts again up to what [`Pool::warm`] asked for, after some were taken.
    pub fn refill(&self) {
        let keys: Vec<Key> = self.slots.lock().unwrap().keys().cloned().collect();
        for key in keys {
            self.top_up(&key);
        }
    }

    /// Keeps the hosts there are but starts no more: for one battle, which has no next one
    /// to start hosts for ([`Pool::refill`] does nothing from here on).
    pub fn no_refill(&self) {
        for slot in self.slots.lock().unwrap().values_mut() {
            slot.target = 0;
        }
    }

    /// The number of hosts started and waiting.
    pub fn idle(&self) -> usize {
        let mut slots = self.slots.lock().unwrap();
        slots
            .values_mut()
            .map(|s| {
                s.ready.retain_mut(Warm::alive);
                s.ready.len()
            })
            .sum()
    }

    /// How many robots were started from the pool's hosts, rather than cold.
    pub fn assigned(&self) -> usize {
        self.assigned.load(Ordering::Relaxed)
    }

    /// The hosts of `key` still to start, started outside the lock.
    fn top_up(&self, key: &Key) {
        let (host, missing) = {
            let mut slots = self.slots.lock().unwrap();
            let Some(slot) = slots.get_mut(key) else {
                return;
            };
            slot.ready.retain_mut(Warm::alive);
            let have = slot.ready.len() + slot.starting;
            let missing = slot.target.saturating_sub(have);
            slot.starting += missing;
            (slot.host.clone(), missing)
        };
        let Some(host) = host else {
            return;
        };
        for _ in 0..missing {
            let started = self.start(&host, key);
            let mut slots = self.slots.lock().unwrap();
            if let Some(slot) = slots.get_mut(key) {
                slot.starting -= 1;
                match started {
                    Ok(warm) => slot.ready.push(warm),
                    // Launches start cold, and say why.
                    Err(e) => robocode_s_server::log_perf(format_args!("warm host: {e}")),
                }
            }
        }
    }

    fn start(&self, host: &JavaHost, key: &Key) -> io::Result<Warm> {
        let home = self.dir.join(format!(
            "robot-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        ));
        // A reused process id's leftovers.
        let _ = std::fs::remove_dir_all(&home);
        let spec = RobotLaunch {
            transport: Transport::Warm,
            robot: String::new(),
            jars: Vec::new(),
            home: home.clone(),
            seed: 0,
            deterministic: key.deterministic,
            memory_mb: key.memory_mb,
        };
        Ok(Warm {
            process: launch(host, &spec)?,
            home,
        })
    }

    /// A started host for a launch of `host` as `memory_mb` and `deterministic` say, if
    /// there is one; the pool doesn't start another, [`Pool::refill`] does.
    fn take(&self, host: &JavaHost, memory_mb: u32, deterministic: bool) -> Option<Warm> {
        let key = Key::new(host, memory_mb, deterministic);
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.get_mut(&key)?;
        while !slot.ready.is_empty() {
            let mut warm = slot.ready.remove(0);
            if warm.alive() {
                return Some(warm);
            }
        }
        None
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let slots = std::mem::take(self.slots.get_mut().unwrap());
        for warm in slots.into_values().flat_map(|s| s.ready) {
            warm.discard();
        }
        // Not a directory with homes of hosts that played: those stay, as battles' do.
        let _ = std::fs::remove_dir(&self.dir);
    }
}

impl Warm {
    /// Whether the host is still running (it may have failed to start).
    fn alive(&mut self) -> bool {
        matches!(self.process.try_wait(), Ok(None))
    }

    /// Stops the host and removes its home.
    fn discard(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }

    /// Gives the host its robot: the jars go into its home, then the assignment.
    fn assign(mut self, spec: &RobotLaunch) -> io::Result<RobotProcess> {
        let staged = self.home.join("jars");
        std::fs::create_dir_all(&staged)?;
        let mut jars = Vec::new();
        for jar in &spec.jars {
            let name = jar.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "a jar without a name")
            })?;
            std::fs::copy(jar, staged.join(name))?;
            // With a separator that is the same in every platform's host.
            jars.push(format!("jars/{}", name.to_string_lossy()));
        }
        let assignment = v1::RobotAssignment {
            robot: spec.robot.clone(),
            seed: spec.seed,
            deterministic: spec.deterministic,
            jars,
        };
        let (_, stdin) = self.process.pipes.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "the warm host's pipes are taken")
        })?;
        write_message(stdin, &assignment)?;
        stdin.flush()?;
        Ok(self.process)
    }
}

/// Starts `spec`'s robot: as a warm host of `pool` if it has one that fits (the launch is
/// over stdio, in jars, not in a class folder, which is read in place and so would need a
/// grant), else cold, as [`launch`] does.
pub fn launch_with(
    pool: Option<&Pool>,
    host: &JavaHost,
    spec: &RobotLaunch,
) -> io::Result<RobotProcess> {
    if let Some(pool) = pool
        && matches!(spec.transport, Transport::Stdio)
        && spec.jars.iter().all(|j| j.is_file())
        && let Some(warm) = pool.take(host, spec.memory_mb, spec.deterministic)
    {
        match warm.assign(spec) {
            Ok(process) => {
                pool.assigned.fetch_add(1, Ordering::Relaxed);
                return Ok(process);
            }
            Err(e) => robocode_s_server::log_perf(format_args!("warm host unusable: {e}")),
        }
    }
    launch(host, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(sandbox: Sandbox) -> JavaHost {
        JavaHost {
            java: "/nonexistent/bin/java".into(),
            lib_dir: "/opt/host/lib".into(),
            agent_jar: "/opt/agent.jar".into(),
            sandbox,
        }
    }

    #[test]
    fn keys_tell_apart_what_a_host_can_not_change_after_it_started() {
        let key = |h: &JavaHost, memory, deterministic| Key::new(h, memory, deterministic);
        let h = host(Sandbox::None);
        assert_eq!(key(&h, 256, true), key(&h.clone(), 256, true));
        assert_ne!(key(&h, 256, true), key(&h, 512, true), "heap");
        assert_ne!(key(&h, 256, true), key(&h, 256, false), "mode");
        assert_ne!(
            key(&h, 256, true),
            key(&host(Sandbox::Os), 256, true),
            "sandbox"
        );
        let other = JavaHost {
            lib_dir: "/elsewhere/lib".into(),
            ..h.clone()
        };
        assert_ne!(key(&h, 256, true), key(&other, 256, true), "robot host");
        let other = JavaHost {
            java: "/other/bin/java".into(),
            ..h.clone()
        };
        assert_ne!(key(&h, 256, true), key(&other, 256, true), "java");
        // The agent only matters where it is loaded.
        let other = JavaHost {
            agent_jar: "/other/agent.jar".into(),
            ..h.clone()
        };
        assert_ne!(key(&h, 256, true), key(&other, 256, true), "agent");
        assert_eq!(key(&h, 256, false), key(&other, 256, false));
    }

    #[test]
    fn an_empty_pool_launches_cold() {
        // The Java isn't there: both ways fail alike, and nothing is taken.
        let dir = std::env::temp_dir().join(format!("robocodes-pool-{}", std::process::id()));
        let pool = Pool::new(dir.clone());
        let h = host(Sandbox::None);
        let spec = RobotLaunch {
            transport: Transport::Stdio,
            robot: "sample.Fire".into(),
            jars: Vec::new(),
            home: dir.join("home"),
            seed: 1,
            deterministic: false,
            memory_mb: 256,
        };
        assert_eq!(pool.idle(), 0);
        assert!(pool.take(&h, 256, false).is_none());
        assert!(launch_with(Some(&pool), &h, &spec).is_err());
        assert!(launch_with(None, &h, &spec).is_err());
        // Hosts that can't start leave the pool empty, not stuck.
        pool.warm(&h, 256, false, 2);
        assert_eq!(pool.idle(), 0);
        pool.refill();
        assert_eq!(pool.idle(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pool_keeps_hosts_of_one_kind() {
        // `cat` stands in for a JVM (and fails on its arguments).
        let Some(cat) = ["/bin/cat", "/usr/bin/cat"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        else {
            return;
        };
        if !cfg!(unix) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("robocodes-pool2-{}", std::process::id()));
        let pool = Pool::new(dir.clone());
        let h = JavaHost {
            java: cat,
            ..host(Sandbox::None)
        };
        pool.warm(&h, 256, false, 2);
        assert_eq!(pool.slots.lock().unwrap().len(), 1);
        // Another kind replaces the first.
        pool.warm(&h, 512, false, 1);
        assert_eq!(pool.slots.lock().unwrap().len(), 1);
        pool.no_refill();
        assert!(pool.slots.lock().unwrap().values().all(|s| s.target == 0));
        drop(pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warm_launches_have_no_robot_on_the_command_line() {
        let h = host(Sandbox::None);
        let spec = RobotLaunch {
            transport: Transport::Warm,
            robot: String::new(),
            jars: Vec::new(),
            home: "/tmp/p/robot-0".into(),
            seed: 0,
            deterministic: true,
            memory_mb: 256,
        };
        let args = crate::jvm_args(&h, &spec);
        let tail = &args[args.iter().position(|a| a == "--stdio").unwrap()..];
        assert_eq!(tail, ["--stdio", "--warm", "--home", "/tmp/p/robot-0"]);
        assert!(args.contains(&"-javaagent:/opt/agent.jar".to_string()));
    }
}
