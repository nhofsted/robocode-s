// SPDX-License-Identifier: EPL-2.0

//! The sandbox robot processes run in (ADR 0019, design §6.1). `--sandbox` has the same
//! values on every platform; each platform has some of them.

#[cfg(target_os = "linux")]
mod linux;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

/// Where a robot process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sandbox {
    /// The operating system's own sandbox: on Linux no new privileges, seccomp and rlimits,
    /// and Landlock where the kernel has it.
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
            Self::Os => Some("the os sandbox isn't there yet on Windows (#69)".into()),
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

    /// What robots in this sandbox run with: on Linux its layers, e.g.
    /// `os (seccomp, Landlock ABI 7)`.
    pub fn report(self) -> String {
        match self {
            #[cfg(target_os = "linux")]
            Self::Os | Self::Bwrap => linux::report(self),
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
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // until Windows' `os` (#69)
pub(crate) struct Grants {
    /// Read and execute: the Java runtime (the directory above its `bin`).
    pub runtime: PathBuf,
    /// Read: the robot host, the agent and the robot's jars and class folders.
    pub read: Vec<PathBuf>,
    /// Read and write: the robot's home, with its data files and temporary files.
    pub home: PathBuf,
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
                    if let Ok(real) = path.canonicalize()
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
fn kept(name: &str) -> bool {
    matches!(name, "PATH" | "HOME" | "LANG" | "LANGUAGE" | "TZ") || name.starts_with("LC_")
}

/// A command that starts `program` in `sandbox` with `grants`; the caller adds the
/// arguments and the rest. `none` is a plain command.
pub(crate) fn command(sandbox: Sandbox, program: &Path, grants: &Grants) -> io::Result<Command> {
    if let Some(why) = sandbox.unavailable() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, why));
    }
    if sandbox == Sandbox::None {
        return Ok(Command::new(program));
    }
    std::fs::create_dir_all(grants.home.join("tmp"))?;
    let mut command = match sandbox {
        #[cfg(target_os = "linux")]
        Sandbox::Os => linux::os_command(program, grants, true)?,
        #[cfg(target_os = "linux")]
        Sandbox::Bwrap => linux::bwrap_command(program, grants)?,
        _ => Command::new(program),
    };
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(k, _)| k.to_str().is_some_and(kept)));
    Ok(command)
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
        if cfg!(target_os = "linux") {
            assert_eq!(os, Ok(Sandbox::Os));
            assert_eq!(Sandbox::default(), Sandbox::Os);
        } else {
            let why = os.unwrap_err();
            assert!(why.contains("here robots run in: none"), "{why}");
            assert!(!cfg!(windows) || why.contains("#69"), "{why}");
            assert_eq!(Sandbox::default(), Sandbox::None);
            let bwrap = "bwrap".parse::<Sandbox>().unwrap_err();
            assert!(bwrap.contains("only on Linux"), "{bwrap}");
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
