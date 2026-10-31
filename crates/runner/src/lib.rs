// SPDX-License-Identifier: EPL-2.0

//! Launches robot processes (design §6): one JVM per robot running the robocode-s robot
//! host (`clients/java-compat`), in a [`Sandbox`] (ADR 0019).

pub mod agent;
pub mod housekeeping;
pub mod ide;
pub mod jre;
pub mod local;
pub mod pool;
pub mod remote;
mod sandbox;

pub use sandbox::{Sandbox, docker_pulling};

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{Arc, Mutex};
use std::thread;

/// Where the Java robot host is installed (`./gradlew :clients:java-compat:installDist`
/// and `:clients:java-determinism:jar`).
#[derive(Debug, Clone)]
pub struct JavaHost {
    /// The `java` executable ([`jre::java`]).
    pub java: PathBuf,
    /// The robot host's `lib` directory: its own jar and classic Robocode's jars.
    pub lib_dir: PathBuf,
    /// The determinism agent jar (ADR 0009).
    pub agent_jar: PathBuf,
    /// What robot hosts run in, battles and listings alike: they load robot code.
    pub sandbox: Sandbox,
}

/// How a launched robot talks to the server.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Over the robot host's stdin and stdout, which the runner takes
    /// ([`RobotProcess::take_pipes`]): the pipes are the robot's identity, and it needs no
    /// network (ADR 0019).
    Stdio,
    /// [`Transport::Stdio`] with a robot host that starts without its robot (`--warm`) and
    /// waits for its assignment on stdin ([`pool`], design §6.2). The launch's robot, jars
    /// and seed are for that assignment, not the command line.
    Warm,
    /// Over TCP, for a robot a server reaches by address.
    Tcp {
        /// Server address, e.g. `127.0.0.1:7654`.
        server: String,
        /// The robot's one-time token. It goes to the robot host in its environment
        /// ([`TOKEN_ENV`]), not on its command line, which other users may read.
        token: String,
    },
}

/// One robot to launch.
#[derive(Debug, Clone)]
pub struct RobotLaunch {
    pub transport: Transport,
    /// Classic robot name to look up in the jars, e.g. `sample.Fire` or `jk.mega.DrussGT 3.1.16`.
    pub robot: String,
    /// Jars or class folders containing the robot.
    pub jars: Vec<PathBuf>,
    /// Working directory (classic's robocode home: robot data files live here).
    pub home: PathBuf,
    /// Battle seed for the robot's own random streams.
    pub seed: i64,
    /// Deterministic mode: StrictMath semantics and seeded randomness (ADR 0003, 0009).
    pub deterministic: bool,
    pub memory_mb: u32,
    /// The robot's persistent data folder ([`housekeeping::robot_data_dir`]): its files
    /// are put in the home before the robot starts and taken back when it has ended
    /// (design §6.5). `None`: the robot starts with none and they are not kept.
    pub data: Option<PathBuf>,
}

/// The environment variable the robot host reads its token from.
pub const TOKEN_ENV: &str = "ROBOCODES_ROBOT_TOKEN";

/// Same module openings as classic's robocode.sh; classic's repository needs them.
pub const MODULE_OPENINGS: [&str; 2] = [
    "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED",
    "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
];

/// JVM flags for a robot host without a sandbox. On macOS it has no display either, as in
/// a sandbox: AWT's toolkit, which classic's classes load, would make each robot's JVM an
/// app with a Dock icon that takes the focus.
#[cfg(target_os = "macos")]
pub const PLAIN_FLAGS: [&str; 1] = ["-Djava.awt.headless=true"];
#[cfg(not(target_os = "macos"))]
pub const PLAIN_FLAGS: [&str; 0] = [];

/// Switches Math.sin, cos, tan, atan2, exp, log, log10 and pow to StrictMath's results: their
/// intrinsics are inexact and differ from it. The exact intrinsics (sqrt, abs, ...) stay on.
pub const STRICT_MATH_FLAGS: [&str; 2] = [
    "-XX:+UnlockDiagnosticVMOptions",
    "-XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow",
];

/// What a robot's JVM looks like from inside, the same on every machine (ADR 0025): its
/// locale (classic's on an English system, as the difftest oracle's), time zone, text
/// encodings and user name. Robots format numbers, read the date or their user name; on a
/// Dutch or a German system `String.format("%.2f")` writes a comma, which `Double.parseDouble`
/// then refuses. Robots in classic mode get it too: their numbers shouldn't depend on the
/// machine either.
pub const NEUTRAL_FLAGS: [&str; 7] = [
    "-Duser.language=en",
    "-Duser.country=US",
    "-Duser.timezone=UTC",
    "-Dfile.encoding=UTF-8",
    "-Dstdout.encoding=UTF-8",
    "-Dstderr.encoding=UTF-8",
    "-Duser.name=robocode-s",
];

/// Deterministic mode (ADR 0025): one processor (`Runtime.availableProcessors()`, the
/// common pool's parallelism, the GC's choice: Serial), and the heap's size fixed from the
/// start, so `totalMemory()` and `freeMemory()` don't follow the machine's RAM. The robot's
/// threads run one at a time anyway. Lines end in `\n` on every platform, in the files a
/// robot writes and reads back too (the agent answers robot code's `System` calls likewise).
pub const DETERMINISTIC_FLAGS: [&str; 2] = ["-XX:ActiveProcessorCount=1", "-Dline.separator=\n"];

/// The JVM arguments for a robot.
pub fn jvm_args(host: &JavaHost, launch: &RobotLaunch) -> Vec<String> {
    let mut args = vec![format!("-Xmx{}m", launch.memory_mb)];
    args.extend(NEUTRAL_FLAGS.map(String::from));
    if launch.deterministic {
        args.push(format!("-Xms{}m", launch.memory_mb));
        args.extend(DETERMINISTIC_FLAGS.map(String::from));
    }
    if host.sandbox != Sandbox::None {
        args.extend(sandbox::jvm_flags(&launch.home));
    } else {
        args.extend(PLAIN_FLAGS.map(String::from));
    }
    args.extend(MODULE_OPENINGS.map(String::from));
    if launch.deterministic {
        args.extend(STRICT_MATH_FLAGS.map(String::from));
        args.push(format!("-javaagent:{}", host.agent_jar.display()));
    }
    args.push("-cp".into());
    args.push(host.lib_dir.join("*").display().to_string());
    args.push("io.github.nhofsted.robocodes.compat.RobotHost".into());
    match &launch.transport {
        Transport::Stdio => args.push("--stdio".into()),
        Transport::Warm => args.extend(["--stdio".into(), "--warm".into()]),
        Transport::Tcp { server, .. } => {
            args.push("--server".into());
            args.push(server.clone());
        }
    }
    args.push("--home".into());
    args.push(launch.home.display().to_string());
    if !matches!(launch.transport, Transport::Warm) {
        for (flag, value) in [
            ("--robot", launch.robot.clone()),
            ("--seed", launch.seed.to_string()),
        ] {
            args.push(flag.into());
            args.push(value);
        }
        for jar in &launch.jars {
            args.push("--jar".into());
            args.push(jar.display().to_string());
        }
    }
    args
}

/// A running robot process.
pub struct RobotProcess {
    child: sandbox::Child,
    pipes: Option<(RobotStdout, RobotStdin)>,
    stderr: Arc<Mutex<String>>,
    /// What to do once it has ended, once.
    cleanup: Option<Cleanup>,
}

/// What follows a robot's end: its data files are taken from its home, which goes.
struct Cleanup {
    home: PathBuf,
    /// The persistent data folder, and whether the robot was given files from it.
    data: Option<(PathBuf, bool)>,
}

/// What a robot writes: its stdout.
pub type RobotStdout = Box<dyn Read + Send>;
/// What a robot reads: its stdin.
pub type RobotStdin = Box<dyn Write + Send>;

/// How much of a robot's error output is kept for diagnosis.
const STDERR_TAIL: usize = 8 * 1024;

/// Starts a robot JVM in the host's sandbox. Paths may be relative to the current
/// directory; the robot runs in its home directory, so they are made absolute first.
pub fn launch(host: &JavaHost, launch: &RobotLaunch) -> io::Result<RobotProcess> {
    if host.sandbox != Sandbox::None && matches!(launch.transport, Transport::Tcp { .. }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a robot in a sandbox has no network: it talks over stdio",
        ));
    }
    std::fs::create_dir_all(&launch.home)?;
    let had_data = match &launch.data {
        Some(data) => housekeeping::copy_data_in(data, &launch.home)?,
        None => false,
    };
    let host = JavaHost {
        java: absolute_if_path(&host.java)?,
        lib_dir: std::path::absolute(&host.lib_dir)?,
        agent_jar: std::path::absolute(&host.agent_jar)?,
        sandbox: host.sandbox,
    };
    let launch = RobotLaunch {
        jars: launch
            .jars
            .iter()
            .map(std::path::absolute)
            .collect::<io::Result<_>>()?,
        home: std::path::absolute(&launch.home)?,
        ..launch.clone()
    };
    let mut read = vec![host.lib_dir.clone()];
    if launch.deterministic {
        read.push(host.agent_jar.clone());
    }
    read.extend(launch.jars.iter().cloned());
    // A container sees the paths mounted at other ones.
    let (seen_host, seen_launch) = if host.sandbox.is_container() {
        container_view(&host, &launch)
    } else {
        (host.clone(), launch.clone())
    };
    let mut sandboxed = sandboxed(&host, read, &launch.home, launch.memory_mb)?;
    no_window(&mut sandboxed.command)
        .args(jvm_args(&seen_host, &seen_launch))
        .current_dir(&launch.home);
    let stdio = match &launch.transport {
        Transport::Stdio | Transport::Warm => true,
        Transport::Tcp { token, .. } => {
            sandboxed.command.env(TOKEN_ENV, token);
            false
        }
    };
    let mut child = sandboxed.spawn(stdio, stdio)?;
    let pipes = child.stdout.take().zip(child.stdin.take());
    let stderr = Arc::new(Mutex::new(String::new()));
    if let Some(pipe) = child.stderr.take() {
        let tail = stderr.clone();
        thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let mut t = tail.lock().unwrap();
                t.push_str(&line);
                t.push('\n');
                if t.len() > STDERR_TAIL {
                    let cut = t.len() - STDERR_TAIL;
                    let cut = (cut..t.len())
                        .find(|&i| t.is_char_boundary(i))
                        .unwrap_or(t.len());
                    t.drain(..cut);
                }
            }
        });
    }
    Ok(RobotProcess {
        child,
        pipes,
        stderr,
        cleanup: Some(Cleanup {
            home: launch.home.clone(),
            data: launch.data.clone().map(|d| (d, had_data)),
        }),
    })
}

impl RobotProcess {
    /// The robot's stdout and stdin of a [`Transport::Stdio`] launch, once.
    pub fn take_pipes(&mut self) -> Option<(RobotStdout, RobotStdin)> {
        self.pipes.take()
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }

    /// Ends the robot if it still runs, keeps its data files and removes its home (unless
    /// `ROBOCODES_KEEP_BATTLES` says to keep it). Done once; dropping the process does it.
    pub fn finish(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };
        if let Some((data, had)) = &cleanup.data
            && let Err(e) = housekeeping::copy_data_out(&cleanup.home, data, *had)
        {
            robocode_s_server::log_perf(format_args!(
                "robot data files in {}: {e}",
                data.display()
            ));
        }
        housekeeping::remove_home(&cleanup.home);
    }

    /// The last part of the robot's error output.
    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }
}

impl Drop for RobotProcess {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Starts `command` without a console window of its own: on Windows a GUI program that
/// starts `java.exe` would otherwise open one for every robot host.
pub(crate) fn no_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The host's `java` in its sandbox, which lets it read `read` and read and write `home`,
/// with a heap of `memory_mb` (`-Xmx`).
pub(crate) fn sandboxed(
    host: &JavaHost,
    mut read: Vec<PathBuf>,
    home: &Path,
    memory_mb: u32,
) -> io::Result<sandbox::Sandboxed> {
    if host.sandbox == Sandbox::None {
        // No sandbox, but the fixed environment all the same (ADR 0025): `java` is found on
        // the user's `PATH` first, as the robot's isn't it.
        let (java, runtime) = sandbox::java_runtime(&host.java)
            .unwrap_or_else(|_| (host.java.clone(), PathBuf::new()));
        let grants = sandbox::Grants {
            runtime,
            read,
            home: home.to_path_buf(),
            memory_mb,
        };
        return sandbox::command(Sandbox::None, &java, &grants);
    }
    if host.sandbox.is_container() {
        // The container has its own Java.
        let grants = sandbox::Grants {
            runtime: PathBuf::new(),
            read,
            home: home.to_path_buf(),
            memory_mb,
        };
        return sandbox::command(host.sandbox, &host.java, &grants);
    }
    let (java, runtime) = sandbox::java_runtime(&host.java)?;
    read.extend(sandbox::linked_from(&runtime));
    let grants = sandbox::Grants {
        runtime,
        read,
        home: home.to_path_buf(),
        memory_mb,
    };
    sandbox::command(host.sandbox, &java, &grants)
}

/// The host and the launch with the paths a container has them at: [`sandbox::container_path`]
/// of the read grants `launch` makes (the robot host's `lib`, the agent if deterministic,
/// the jars).
fn container_view(host: &JavaHost, launch: &RobotLaunch) -> (JavaHost, RobotLaunch) {
    let agent = usize::from(launch.deterministic);
    let seen = JavaHost {
        lib_dir: sandbox::container_path(0, &host.lib_dir),
        agent_jar: sandbox::container_path(1, &host.agent_jar),
        ..host.clone()
    };
    let jars = launch.jars.iter().enumerate();
    let seen_launch = RobotLaunch {
        jars: jars
            .map(|(i, jar)| sandbox::container_path(1 + agent + i, jar))
            .collect(),
        home: PathBuf::from(sandbox::CONTAINER_HOME),
        ..launch.clone()
    };
    (seen, seen_launch)
}

/// A relative executable path with a directory part becomes absolute; a bare name like
/// `java` is left for the PATH lookup.
fn absolute_if_path(p: &Path) -> io::Result<PathBuf> {
    if p.components().count() > 1 {
        std::path::absolute(p)
    } else {
        Ok(p.to_path_buf())
    }
}

/// A fresh home directory for one robot of one battle.
pub fn robot_home(base: &Path, battle: &str, robot_index: usize) -> PathBuf {
    base.join(battle).join(format!("robot-{robot_index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_robots_get_strict_math_and_the_agent() {
        let host = JavaHost {
            java: "java".into(),
            lib_dir: "/opt/host/lib".into(),
            agent_jar: "/opt/agent.jar".into(),
            sandbox: Sandbox::None,
        };
        let mut launch = RobotLaunch {
            transport: Transport::Tcp {
                server: "127.0.0.1:1".into(),
                token: "t".into(),
            },
            robot: "sample.Fire".into(),
            jars: vec!["/r/samples.jar".into()],
            home: "/tmp/h".into(),
            seed: 42,
            deterministic: true,
            memory_mb: 256,
            data: None,
        };
        let args = jvm_args(&host, &launch);
        assert!(args.contains(&STRICT_MATH_FLAGS[1].to_string()));
        for flag in NEUTRAL_FLAGS.iter().chain(&DETERMINISTIC_FLAGS) {
            assert!(args.contains(&flag.to_string()), "{flag}");
        }
        assert!(args.contains(&"-Xms256m".to_string()) && args.contains(&"-Xmx256m".to_string()));
        assert!(args.contains(&"-javaagent:/opt/agent.jar".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "--seed" && w[1] == "42"));
        assert!(!args.iter().any(|a| a == "--token" || a == "t"));
        assert!(args.contains(&"--server".to_string()));
        launch.transport = Transport::Stdio;
        let args = jvm_args(&host, &launch);
        assert!(args.contains(&"--stdio".to_string()));
        assert!(!args.contains(&"--server".to_string()));
        launch.deterministic = false;
        let args = jvm_args(&host, &launch);
        assert!(!args.iter().any(|a| a.contains("DisableIntrinsic")
            || a.contains("javaagent")
            || a.contains("ActiveProcessorCount")
            || a.starts_with("-Xms")));
        assert!(args.contains(&"-Duser.timezone=UTC".to_string()));
        // Without a sandbox it may have a display, except on macOS (a Dock icon each).
        assert_eq!(
            args.iter().any(|a| a.contains("headless")),
            cfg!(target_os = "macos")
        );
        // A sandboxed robot host has no display and its temporary files in its home.
        let host = JavaHost {
            sandbox: Sandbox::Os,
            ..host
        };
        let args = jvm_args(&host, &launch);
        assert!(args.contains(&"-Djava.awt.headless=true".to_string()));
        let tmp = Path::new("/tmp/h").join("tmp");
        assert!(args.contains(&format!("-Djava.io.tmpdir={}", tmp.display())));
    }
}
