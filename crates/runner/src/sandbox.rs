// SPDX-License-Identifier: EPL-2.0

//! The sandbox robot processes run in (ADR 0019, design §6.1). `--sandbox` has the same
//! values on every platform; each platform has some of them.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::str::FromStr;

/// Where a robot process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sandbox {
    /// The operating system's own sandbox: on Linux no new privileges, seccomp and rlimits,
    /// and Landlock where the kernel has it; on Windows an AppContainer and a Job Object.
    Os,
    /// Bubblewrap, where it is installed (Linux): namespaces that show the robot only its
    /// grants, with the seccomp filter and rlimits of `Os`.
    Bwrap,
    /// A container (#70).
    Docker,
    /// A plain process.
    None,
}

impl Sandbox {
    pub const ALL: [Sandbox; 4] = [Self::Os, Self::Bwrap, Self::Docker, Self::None];

    pub fn name(self) -> &'static str {
        match self {
            Self::Os => "os",
            Self::Bwrap => "bwrap",
            Self::Docker => "docker",
            Self::None => "none",
        }
    }

    /// Why robots can't run in this sandbox on this platform; `None` if they can.
    pub fn unavailable(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Docker => Some("the docker sandbox isn't there yet (#70)".into()),
            #[cfg(target_os = "linux")]
            Self::Os => linux::os_unavailable(),
            #[cfg(target_os = "linux")]
            Self::Bwrap => linux::bwrap_unavailable(),
            #[cfg(windows)]
            Self::Os => windows::os_unavailable(),
            #[cfg(not(any(target_os = "linux", windows)))]
            Self::Os => Some("this platform has no os sandbox".into()),
            #[cfg(not(target_os = "linux"))]
            Self::Bwrap => Some("bwrap is only on Linux".into()),
        }
    }

    /// The sandboxes robots can run in here.
    pub fn available() -> Vec<Sandbox> {
        Self::ALL
            .into_iter()
            .filter(|s| s.unavailable().is_none())
            .collect()
    }

    /// What robots in this sandbox run with: its layers, e.g.
    /// `os (seccomp, Landlock ABI 7)` on Linux, `os (AppContainer, Job Object)` on Windows.
    pub fn report(self) -> String {
        match self {
            #[cfg(target_os = "linux")]
            Self::Os | Self::Bwrap => linux::report(self),
            #[cfg(windows)]
            Self::Os => "os (AppContainer, Job Object)".into(),
            _ => self.name().into(),
        }
    }
}

impl Default for Sandbox {
    /// `os` where the platform has it, else `none`.
    fn default() -> Self {
        if Self::Os.unavailable().is_none() {
            Self::Os
        } else {
            Self::None
        }
    }
}

impl fmt::Display for Sandbox {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Sandbox names, e.g. `os, bwrap, none`.
fn names(sandboxes: &[Sandbox]) -> String {
    sandboxes
        .iter()
        .map(|s| s.name())
        .collect::<Vec<_>>()
        .join(", ")
}

impl FromStr for Sandbox {
    type Err = String;

    /// A sandbox by name, if this platform has it; the error names the ones it has.
    fn from_str(s: &str) -> Result<Self, String> {
        let sandbox = Self::ALL
            .into_iter()
            .find(|k| k.name() == s)
            .ok_or_else(|| format!("unknown sandbox {s} (one of {})", names(&Self::ALL)))?;
        match sandbox.unavailable() {
            None => Ok(sandbox),
            Some(why) => Err(format!(
                "{why}; here robots run in: {}",
                names(&Self::available())
            )),
        }
    }
}

/// What a sandboxed program may use besides the system's libraries and settings.
#[derive(Debug, Clone)]
pub(crate) struct Grants {
    /// Read and execute: the Java runtime (the directory above its `bin`).
    pub runtime: PathBuf,
    /// Read: the robot host, the agent and the robot's jars and class folders.
    pub read: Vec<PathBuf>,
    /// Read and write: the robot's home, with its data files and temporary files.
    pub home: PathBuf,
    /// The JVM's heap (`-Xmx`): Windows' job lets the process commit that and some more.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub memory_mb: u32,
}

/// The `java` executable `java` (a path, or a name on the PATH) and the runtime it is in.
pub(crate) fn java_runtime(java: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let exe = if java.components().count() > 1 {
        java.to_path_buf()
    } else {
        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(java))
                    .find(|p| p.is_file())
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} not found", java.display()),
                )
            })?
    };
    let real = exe.canonicalize()?;
    // Windows' canonical paths are verbatim (`\\?\C:\...`); Java takes the plain ones.
    #[cfg(windows)]
    let real = windows::plain(&real);
    let runtime = real
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: not in a Java runtime's bin", real.display()),
            )
        })?
        .to_path_buf();
    Ok((real, runtime))
}

/// What links in `runtime` (a real path) lead to outside it, to be read too: a
/// distribution's Java keeps its settings elsewhere (Debian's `conf` files are links into
/// `/etc/java-21-openjdk`, `lib/security/cacerts` into `/etc/ssl`).
pub(crate) fn linked_from(runtime: &Path) -> Vec<PathBuf> {
    // A runtime has hundreds of files; this bounds a strange one.
    const ENTRIES: usize = 20_000;
    let mut seen = 0;
    let mut found = Vec::new();
    let mut stack = vec![runtime.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return found;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_symlink() => {
                    #[cfg(windows)]
                    let real = path.canonicalize().map(|p| windows::plain(&p));
                    #[cfg(not(windows))]
                    let real = path.canonicalize();
                    if let Ok(real) = real
                        && !real.starts_with(runtime)
                        && !found.contains(&real)
                    {
                        found.push(real);
                    }
                }
                Ok(t) if t.is_dir() => stack.push(path),
                _ => {}
            }
        }
    }
    found
}

/// JVM flags for a robot host in a sandbox: no display (as without `DISPLAY`: the robot
/// can't reach it), temporary files in its home, and no performance data in `/tmp`.
pub(crate) fn jvm_flags(home: &Path) -> [String; 3] {
    [
        "-Djava.awt.headless=true".into(),
        "-XX:-UsePerfData".into(),
        format!("-Djava.io.tmpdir={}", home.join("tmp").display()),
    ]
}

/// The environment variables a sandboxed program keeps: the user's others may hold
/// secrets (tokens, keys), and the display's.
#[cfg(not(windows))]
fn kept(name: &str) -> bool {
    matches!(name, "PATH" | "HOME" | "LANG" | "LANGUAGE" | "TZ") || name.starts_with("LC_")
}

/// On Windows, whatever their case, with what its programs need: the system's folders
/// and the processors. `PATH`, `TEMP` and `TMP` are the sandbox's own.
#[cfg(windows)]
fn kept(name: &str) -> bool {
    const KEPT: [&str; 10] = [
        "SystemRoot",
        "windir",
        "SystemDrive",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "LOCALAPPDATA",
        "HOME",
        "LANG",
        "LANGUAGE",
        "TZ",
    ];
    KEPT.iter().any(|k| k.eq_ignore_ascii_case(name))
        || name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("LC_"))
}

/// A program to start in a sandbox ([`command`]): its arguments, environment and
/// directory go on `command`, [`Sandboxed::spawn`] and [`Sandboxed::output`] start it.
/// Windows' `os` can't be started by `Command`: the runner starts it itself.
pub(crate) struct Sandboxed {
    pub command: Command,
    #[cfg(windows)]
    contained: Option<Grants>,
}

impl Sandboxed {
    /// A plain process (`none`).
    pub fn plain(program: &Path) -> Self {
        Self {
            command: Command::new(program),
            #[cfg(windows)]
            contained: None,
        }
    }

    /// Starts the program with its stdin and stdout piped or else null, and its stderr
    /// piped.
    pub fn spawn(mut self, stdin: bool, stdout: bool) -> io::Result<Child> {
        #[cfg(windows)]
        if let Some(grants) = &self.contained {
            let (process, [stdin, stdout, stderr]) =
                windows::spawn(&self.command, grants, [stdin, stdout])?;
            let boxed = |pipe: Option<std::fs::File>| pipe.map(Box::new);
            return Ok(Child {
                process: Process::Contained(process),
                stdin: boxed(stdin).map(|p| p as Box<dyn Write + Send>),
                stdout: boxed(stdout).map(|p| p as Box<dyn Read + Send>),
                stderr: boxed(stderr).map(|p| p as Box<dyn Read + Send>),
            });
        }
        let null_or_piped = |piped| if piped { Stdio::piped() } else { Stdio::null() };
        let mut child = self
            .command
            .stdin(null_or_piped(stdin))
            .stdout(null_or_piped(stdout))
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(Child {
            stdin: child.stdin.take().map(|p| Box::new(p) as _),
            stdout: child.stdout.take().map(|p| Box::new(p) as _),
            stderr: child.stderr.take().map(|p| Box::new(p) as _),
            process: Process::Plain(child),
        })
    }

    /// Runs the program to its end, its stdin null, and returns its output.
    pub fn output(mut self) -> io::Result<Output> {
        #[cfg(windows)]
        if self.contained.is_some() {
            let mut child = self.spawn(false, true)?;
            let read = |pipe: Option<Box<dyn Read + Send>>| {
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    if let Some(mut pipe) = pipe {
                        let _ = pipe.read_to_end(&mut bytes);
                    }
                    bytes
                })
            };
            let (stdout, stderr) = (read(child.stdout.take()), read(child.stderr.take()));
            let status = child.wait()?;
            return Ok(Output {
                status,
                stdout: stdout.join().unwrap_or_default(),
                stderr: stderr.join().unwrap_or_default(),
            });
        }
        self.command.stdin(Stdio::null()).output()
    }
}

/// A started program; its pipes are for the caller to take.
pub(crate) struct Child {
    process: Process,
    pub stdin: Option<Box<dyn Write + Send>>,
    pub stdout: Option<Box<dyn Read + Send>>,
    pub stderr: Option<Box<dyn Read + Send>>,
}

enum Process {
    Plain(std::process::Child),
    #[cfg(windows)]
    Contained(windows::Process),
}

impl Child {
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        match &mut self.process {
            Process::Plain(p) => p.wait(),
            #[cfg(windows)]
            Process::Contained(p) => p.wait(),
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match &mut self.process {
            Process::Plain(p) => p.try_wait(),
            #[cfg(windows)]
            Process::Contained(p) => p.try_wait(),
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        match &mut self.process {
            Process::Plain(p) => p.kill(),
            #[cfg(windows)]
            Process::Contained(p) => p.kill(),
        }
    }
}

/// `program` in `sandbox` with `grants`; the caller adds the arguments and the rest.
/// `none` is a plain process.
pub(crate) fn command(sandbox: Sandbox, program: &Path, grants: &Grants) -> io::Result<Sandboxed> {
    if let Some(why) = sandbox.unavailable() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, why));
    }
    if sandbox == Sandbox::None {
        return Ok(Sandboxed::plain(program));
    }
    std::fs::create_dir_all(grants.home.join("tmp"))?;
    let mut sandboxed = match sandbox {
        #[cfg(target_os = "linux")]
        Sandbox::Os => Sandboxed {
            command: linux::os_command(program, grants, true)?,
        },
        #[cfg(target_os = "linux")]
        Sandbox::Bwrap => Sandboxed {
            command: linux::bwrap_command(program, grants)?,
        },
        #[cfg(windows)]
        Sandbox::Os => Sandboxed {
            command: Command::new(program),
            contained: Some(grants.clone()),
        },
        _ => Sandboxed::plain(program),
    };
    sandboxed
        .command
        .env_clear()
        .envs(std::env::vars_os().filter(|(k, _)| k.to_str().is_some_and(kept)));
    #[cfg(windows)]
    windows::environment(&mut sandboxed.command, grants);
    Ok(sandboxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_refuse_the_sandboxes_they_lack() {
        assert_eq!("none".parse(), Ok(Sandbox::None));
        let docker = "docker".parse::<Sandbox>().unwrap_err();
        assert!(
            docker.contains("#70") && docker.contains("none"),
            "{docker}"
        );
        assert!(
            "chroot"
                .parse::<Sandbox>()
                .unwrap_err()
                .contains("os, bwrap, docker, none")
        );
        let os = "os".parse::<Sandbox>();
        if cfg!(any(target_os = "linux", windows)) {
            assert_eq!(os, Ok(Sandbox::Os));
            assert_eq!(Sandbox::default(), Sandbox::Os);
        } else {
            let why = os.unwrap_err();
            assert!(why.contains("here robots run in: none"), "{why}");
            assert_eq!(Sandbox::default(), Sandbox::None);
        }
        if !cfg!(target_os = "linux") {
            let bwrap = "bwrap".parse::<Sandbox>().unwrap_err();
            assert!(bwrap.contains("only on Linux"), "{bwrap}");
            assert!(bwrap.ends_with("here robots run in: os, none") == cfg!(windows));
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_runtime_are_granted() {
        let dir = std::env::temp_dir().join(format!("robocode-s-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("jre/conf/security")).unwrap();
        std::fs::create_dir_all(dir.join("etc")).unwrap();
        std::fs::write(dir.join("etc/java.security"), "").unwrap();
        std::fs::write(dir.join("jre/release"), "").unwrap();
        let link = |to: &str, at: &str| std::os::unix::fs::symlink(dir.join(to), dir.join(at));
        link("etc/java.security", "jre/conf/security/java.security").unwrap();
        link("jre/release", "jre/conf/release").unwrap();
        let dir = dir.canonicalize().unwrap();
        assert_eq!(
            linked_from(&dir.join("jre")),
            [dir.join("etc/java.security")]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
