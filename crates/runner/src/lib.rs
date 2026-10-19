// SPDX-License-Identifier: EPL-2.0

//! Launches robot processes (design §6). For now the local-process launcher: one JVM per
//! robot running the robocode-s robot host (`clients/java-compat`).

pub mod jre;
pub mod local;

use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
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
}

/// How a launched robot talks to the server.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Over the robot host's stdin and stdout, which the runner takes
    /// ([`RobotProcess::take_pipes`]): the pipes are the robot's identity, and it needs no
    /// network (ADR 0019).
    Stdio,
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
}

/// The environment variable the robot host reads its token from.
pub const TOKEN_ENV: &str = "ROBOCODES_ROBOT_TOKEN";

/// Same module openings as classic's robocode.sh; classic's repository needs them.
pub const MODULE_OPENINGS: [&str; 2] = [
    "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED",
    "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
];

/// Switches Math.sin, cos, tan, atan2, exp, log, log10 and pow to StrictMath's results: their
/// intrinsics are inexact and differ from it. The exact intrinsics (sqrt, abs, ...) stay on.
pub const STRICT_MATH_FLAGS: [&str; 2] = [
    "-XX:+UnlockDiagnosticVMOptions",
    "-XX:DisableIntrinsic=_dsin,_dcos,_dtan,_datan2,_dexp,_dlog,_dlog10,_dpow",
];

/// The JVM arguments for a robot.
pub fn jvm_args(host: &JavaHost, launch: &RobotLaunch) -> Vec<String> {
    let mut args = vec![format!("-Xmx{}m", launch.memory_mb)];
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
        Transport::Tcp { server, .. } => {
            args.push("--server".into());
            args.push(server.clone());
        }
    }
    for (flag, value) in [
        ("--robot", launch.robot.clone()),
        ("--home", launch.home.display().to_string()),
        ("--seed", launch.seed.to_string()),
    ] {
        args.push(flag.into());
        args.push(value);
    }
    for jar in &launch.jars {
        args.push("--jar".into());
        args.push(jar.display().to_string());
    }
    args
}

/// A running robot process.
pub struct RobotProcess {
    child: Child,
    pipes: Option<(ChildStdout, ChildStdin)>,
    stderr: Arc<Mutex<String>>,
}

/// How much of a robot's error output is kept for diagnosis.
const STDERR_TAIL: usize = 8 * 1024;

/// Starts a robot JVM. Paths may be relative to the current directory; the robot runs
/// in its home directory, so they are made absolute first.
pub fn launch(host: &JavaHost, launch: &RobotLaunch) -> io::Result<RobotProcess> {
    std::fs::create_dir_all(&launch.home)?;
    let host = JavaHost {
        java: absolute_if_path(&host.java)?,
        lib_dir: std::path::absolute(&host.lib_dir)?,
        agent_jar: std::path::absolute(&host.agent_jar)?,
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
    let mut command = Command::new(&host.java);
    no_window(&mut command)
        .args(jvm_args(&host, &launch))
        .current_dir(&launch.home)
        .stderr(Stdio::piped());
    match &launch.transport {
        Transport::Stdio => command.stdin(Stdio::piped()).stdout(Stdio::piped()),
        Transport::Tcp { token, .. } => command
            .env(TOKEN_ENV, token)
            .stdin(Stdio::null())
            .stdout(Stdio::null()),
    };
    let mut child = command.spawn()?;
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
    })
}

impl RobotProcess {
    /// The robot's stdout and stdin of a [`Transport::Stdio`] launch, once.
    pub fn take_pipes(&mut self) -> Option<(ChildStdout, ChildStdin)> {
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

    /// The last part of the robot's error output.
    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }
}

impl Drop for RobotProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
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
        };
        let args = jvm_args(&host, &launch);
        assert!(args.contains(&STRICT_MATH_FLAGS[1].to_string()));
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
        assert!(
            !args
                .iter()
                .any(|a| a.contains("DisableIntrinsic") || a.contains("javaagent"))
        );
    }
}
