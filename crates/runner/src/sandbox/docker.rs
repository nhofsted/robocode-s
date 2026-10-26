// SPDX-License-Identifier: EPL-2.0

//! The Docker and Podman sandboxes (ADR 0019, design §6.2): the robot host runs in the
//! official Eclipse Temurin JRE image, pinned by digest, with the robot host, the agent and
//! the robot's jars mounted read-only from the host and the robot's home writable. No
//! network, a read-only root, no capabilities and limits on memory and processes. Its stdio
//! carries the protocol, as for the other sandboxes. Podman takes Docker's command line;
//! [`Engine`] has what differs.
//!
//! Killing the `docker` (`podman`) program doesn't stop the container: [`kill`] does.

use super::Grants;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

/// The digest of the pinned Temurin JRE image: the multi-arch index's (amd64 and arm64).
macro_rules! digest {
    () => {
        "sha256:227118764cccd3c3294617061aaa93a08309c63a395eefa83e6b35032992ea99"
    };
}

/// The pinned Temurin JRE, the release of [`crate::jre::RELEASE`] (an Ubuntu image: glibc,
/// as the downloaded JRE is): updating the JRE means updating the tag and the digest.
pub(crate) const IMAGE: &str = concat!("eclipse-temurin:21.0.12.1_1-jre@", digest!());

/// The same image for Podman, which wants the registry named (it won't ask which one
/// without a terminal) and the digest alone (older ones refuse a tag beside it).
const PODMAN_IMAGE: &str = concat!("docker.io/library/eclipse-temurin@", digest!());

/// Marks the runner's containers.
const LABEL: &str = "robocode-s=1";

/// Where the robot's home is in the container, with a closing separator: paths are joined
/// to it (`tmp`), and on Windows `PathBuf::join` would put a `\` in a container path.
pub(crate) const HOME: &str = "/home/robot/";

/// Where the robot host's `lib` is in the container (closing separator: see [`HOME`]).
const LIB: &str = "/opt/robocode-s/lib/";

/// The processes (and threads) a container may have: the JVM's own are a few dozen.
const PIDS: u32 = 512;

/// How long `docker info` may take.
const PROBE: Duration = Duration::from_secs(10);

/// The program that runs the containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Engine {
    Docker,
    Podman,
}

/// What [`probe`] found of an engine that answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Found {
    /// Podman without root: the container's user is the runner's (`keep-id`).
    rootless: bool,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }

    fn image(self) -> &'static str {
        match self {
            Self::Docker => IMAGE,
            Self::Podman => PODMAN_IMAGE,
        }
    }

    /// The engine's program, with no window on Windows.
    fn command(self) -> Command {
        let mut command = Command::new(program(self.name()));
        crate::no_window(&mut command);
        command
    }

    /// What `info --format` prints: the containers' OS, and for Podman whether it runs
    /// without root.
    fn info_format(self) -> &'static str {
        match self {
            Self::Docker => "{{.OSType}}",
            Self::Podman => "{{.Host.OS}} {{.Host.Security.Rootless}}",
        }
    }

    /// What the engine said when asked, once.
    fn found(self) -> &'static Result<Found, String> {
        static DOCKER: OnceLock<Result<Found, String>> = OnceLock::new();
        static PODMAN: OnceLock<Result<Found, String>> = OnceLock::new();
        match self {
            Self::Docker => DOCKER.get_or_init(|| probe(self)),
            Self::Podman => PODMAN.get_or_init(|| probe(self)),
        }
    }
}

/// `name` on the PATH, or else (an app started from the Finder has only the system's
/// PATH) where macOS installers put it: Docker Desktop's `/usr/local/bin`, Homebrew's
/// `/opt/homebrew/bin` and the Podman installer's `/opt/podman/bin`.
fn program(name: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let on_path = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(name).is_file()));
        if !on_path
            && let Some(found) = ["/usr/local/bin", "/opt/homebrew/bin", "/opt/podman/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(name))
                .find(|p| p.is_file())
        {
            return found;
        }
    }
    PathBuf::from(name)
}

/// Why `engine` can't run robots here: no program, no engine answering, or an engine for
/// Windows containers. Asked once.
pub(super) fn unavailable(engine: Engine) -> Option<String> {
    engine.found().as_ref().err().cloned()
}

fn probe(engine: Engine) -> Result<Found, String> {
    let name = engine.name();
    let mut child = match engine
        .command()
        .args(["info", "--format", engine.info_format()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!("{name} isn't installed (no {name} on the PATH)"));
        }
        Err(e) => return Err(format!("{name}: {e}")),
    };
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < PROBE => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} doesn't answer (`{name} info` timed out)"));
            }
        }
    };
    let output = child
        .wait_with_output()
        .map_err(|e| format!("{name}: {e}"))?;
    refusal(
        engine,
        status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// What `engine` is, or why it can't run robots, from what `info --format` did.
fn refusal(engine: Engine, succeeded: bool, stdout: &str, stderr: &str) -> Result<Found, String> {
    let name = engine.name();
    if !succeeded {
        let why = stderr.trim().lines().next();
        let why = why.map_or(format!("`{name} info` failed"), String::from);
        return Err(format!("{name} isn't running: {why}"));
    }
    let mut words = stdout.split_whitespace();
    let os = words.next().unwrap_or_default();
    if os != "linux" {
        return Err(format!(
            "{name} runs {os} containers; the robot host needs Linux ones"
        ));
    }
    Ok(Found {
        rootless: engine == Engine::Podman && words.next() == Some("true"),
    })
}

static PULLING: AtomicBool = AtomicBool::new(false);

/// Whether the image is being pulled (the first use of `docker` or `podman`, a few hundred
/// MB), to say so where the user waits.
pub fn pulling() -> bool {
    PULLING.load(Ordering::Relaxed)
}

/// Makes sure the image is here, pulling it if not, and removes the runner's stopped
/// containers that an earlier runner left (once). Containers still running are not
/// stray: they stop when their stdin closes, i.e. when the runner that started them is
/// gone.
pub(crate) fn prepare(engine: Engine) -> Result<(), String> {
    static CLEANED: [Once; 2] = [Once::new(), Once::new()];
    CLEANED[engine as usize].call_once(|| {
        let _ = engine
            .command()
            .args(["container", "prune", "--force", "--filter"])
            .arg(format!("label={LABEL}"))
            .args(["--filter", "until=10m"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
    let (name, image) = (engine.name(), engine.image());
    let present = engine
        .command()
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if present {
        return Ok(());
    }
    eprintln!("{name}: pulling {image} (the first time)");
    PULLING.store(true, Ordering::Relaxed);
    let output = engine
        .command()
        .args(["pull", "--quiet", image])
        .stdin(Stdio::null())
        .output();
    PULLING.store(false, Ordering::Relaxed);
    match output {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "{name} pull {image}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("{name} pull: {e}")),
    }
}

/// The path in the container of the `i`th path of [`Grants::read`] (the robot host's
/// `lib`, then the agent and the robot's jars): the same name in a folder of its own.
pub(crate) fn container_path(i: usize, host: &Path) -> PathBuf {
    if i == 0 {
        return PathBuf::from(LIB);
    }
    let name = host
        .file_name()
        .map_or("root".into(), |n| n.to_string_lossy());
    PathBuf::from(format!("/opt/robocode-s/read/{i}/{name}"))
}

/// The container's name and the `docker run` (`podman run`) command up to the Java
/// executable, whose arguments the caller adds.
pub(super) fn command(engine: Engine, grants: &Grants) -> io::Result<(Command, String)> {
    let name = format!("robocode-s-{}", &robocode_s_server::random_token()[..16]);
    let args = run_args(engine, &name, &user(engine, &grants.home)?, grants)?;
    let mut command = engine.command();
    command.args(args);
    Ok((command, name))
}

/// Who the container runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
enum User {
    /// `uid:gid`.
    Id(String),
    /// The runner's user, mapped into rootless Podman's user namespace (`--userns
    /// keep-id`, which also starts the container as that user).
    KeepId,
}

/// The user the container runs as, so that the files the robot writes in its home are the
/// user's: rootless Podman maps the runner's user in; otherwise the owner of the home (the
/// runner's user) on Linux. Elsewhere (Docker Desktop shows mounted files to any user) a
/// fixed one without rights.
fn user(engine: Engine, home: &Path) -> io::Result<User> {
    if let Ok(Found { rootless: true }) = engine.found() {
        return Ok(User::KeepId);
    }
    user_id(home).map(User::Id)
}

fn user_id(home: &Path) -> io::Result<String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = home.metadata()?;
        Ok(format!("{}:{}", owner.uid(), owner.gid()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = home;
        Ok("10001:10001".into())
    }
}

/// `docker run` (`podman run`) for `name`, as `user`, up to the image and `java`.
fn run_args(engine: Engine, name: &str, user: &User, grants: &Grants) -> io::Result<Vec<OsString>> {
    // The heap and the JVM's metaspace, code, threads and GC structures (as Windows' job).
    let memory = format!("{}m", grants.memory_mb + 512);
    let mut args: Vec<OsString> = [
        "run",
        "--interactive",
        "--rm",
        "--init",
        "--name",
        name,
        "--label",
        LABEL,
        "--network",
        "none",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=64m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        &PIDS.to_string(),
        "--memory",
        &memory,
        "--memory-swap",
        &memory,
        // As the other sandboxes' rlimits. The robot's output is on the pipe, not in a log.
        "--ulimit",
        "core=0",
        "--ulimit",
        "nofile=4096:4096",
        "--ulimit",
        "fsize=67108864",
        "--log-driver",
        "none",
        "--workdir",
        HOME,
        "--env",
        &format!("HOME={HOME}"),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.extend(match user {
        User::Id(id) => ["--user".into(), id.into()],
        User::KeepId => ["--userns".into(), "keep-id".into()],
    });
    for (i, host) in grants.read.iter().enumerate() {
        args.extend([
            "--mount".into(),
            mount(host, &container_path(i, host), false)?,
        ]);
    }
    args.extend([
        "--mount".into(),
        mount(&grants.home, Path::new(HOME), true)?,
    ]);
    args.extend([engine.image().into(), "java".into()]);
    Ok(args)
}

/// A bind mount. `--mount` rather than `-v`: a Windows path has a colon of its own.
fn mount(host: &Path, container: &Path, write: bool) -> io::Result<OsString> {
    let bad = |what: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: {what} can't be mounted in a container", host.display()),
        )
    };
    let host = host
        .to_str()
        .ok_or_else(|| bad("a path that isn't UTF-8"))?;
    if host.contains('"') {
        return Err(bad("a path with a quote"));
    }
    // The option list is CSV: a field with a comma is quoted.
    let source = format!("source={host}");
    let source = if host.contains(',') {
        format!("\"{source}\"")
    } else {
        source
    };
    let container = container.display();
    let mode = if write { "" } else { ",readonly" };
    Ok(format!("type=bind,{source},target={container}{mode}").into())
}

/// Stops the container `name` (killing the `docker` program doesn't); it is removed with
/// `--rm`. Without effect once it has exited.
pub(super) fn kill(engine: Engine, name: &str) {
    let _ = engine
        .command()
        .args(["kill", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::Sandboxed;

    fn args(read: &[&str], home: &str) -> Vec<String> {
        engine_args(Engine::Docker, &User::Id("1000:1000".into()), read, home)
    }

    fn engine_args(engine: Engine, user: &User, read: &[&str], home: &str) -> Vec<String> {
        let grants = Grants {
            runtime: PathBuf::new(),
            read: read.iter().map(PathBuf::from).collect(),
            home: home.into(),
            memory_mb: 256,
        };
        run_args(engine, "robocode-s-x", user, &grants)
            .unwrap()
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect()
    }

    fn has(args: &[String], pair: [&str; 2]) -> bool {
        args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1])
    }

    #[test]
    fn a_container_has_no_network_and_little_else() {
        let a = args(&["/h/lib", "/h/agent.jar", "/h/samples.jar"], "/h/home");
        for pair in [
            ["--network", "none"],
            ["--cap-drop", "ALL"],
            ["--security-opt", "no-new-privileges"],
            ["--memory", "768m"],
            ["--memory-swap", "768m"],
            ["--user", "1000:1000"],
            ["--workdir", HOME],
            ["--name", "robocode-s-x"],
        ] {
            assert!(has(&a, pair), "{pair:?} in {a:?}");
        }
        for flag in ["--read-only", "--rm", "--init", "--interactive"] {
            assert!(a.contains(&flag.to_string()), "{flag} in {a:?}");
        }
        // Nothing but the mounts below, and none of the host's environment.
        for flag in [
            "-v",
            "--volume",
            "--privileged",
            "--env-file",
            "--network=host",
        ] {
            assert!(!a.iter().any(|x| x == flag), "{flag} in {a:?}");
        }
        assert_eq!(a.iter().filter(|x| *x == "--env").count(), 1);
        assert!(a.ends_with(&[IMAGE.to_string(), "java".to_string()]));
    }

    #[test]
    fn the_robot_host_and_jars_are_read_only_and_the_home_is_not() {
        let a = args(
            &["/h/lib", "/h/agent.jar", "/h/robots/samples.jar"],
            "/h/home",
        );
        let mounts: Vec<&String> = a.iter().filter(|x| x.starts_with("type=bind")).collect();
        assert_eq!(
            mounts,
            [
                "type=bind,source=/h/lib,target=/opt/robocode-s/lib/,readonly",
                "type=bind,source=/h/agent.jar,target=/opt/robocode-s/read/1/agent.jar,readonly",
                "type=bind,source=/h/robots/samples.jar,target=/opt/robocode-s/read/2/samples.jar,readonly",
                "type=bind,source=/h/home,target=/home/robot/",
            ]
        );
    }

    #[test]
    fn windows_paths_are_mounted_as_they_are() {
        let m = |host: &str, container: &str, write| {
            mount(Path::new(host), Path::new(container), write)
                .unwrap()
                .into_string()
                .unwrap()
        };
        assert_eq!(
            m(r"C:\Users\me\app\lib", LIB, false),
            r"type=bind,source=C:\Users\me\app\lib,target=/opt/robocode-s/lib/,readonly"
        );
        assert_eq!(
            m(r"C:\Users\me\data\home", HOME, true),
            r"type=bind,source=C:\Users\me\data\home,target=/home/robot/"
        );
        // A comma in a path: the field is quoted.
        assert_eq!(
            m(r"C:\Users\me\my,robots\r.jar", "/opt/r.jar", false),
            r#"type=bind,"source=C:\Users\me\my,robots\r.jar",target=/opt/r.jar,readonly"#
        );
        let bad = mount(Path::new(r#"C:\a"b"#), Path::new(HOME), true).unwrap_err();
        assert!(bad.to_string().contains("quote"), "{bad}");
        if cfg!(windows) {
            assert_eq!(
                container_path(1, Path::new(r"C:\Users\me\r.jar")),
                Path::new("/opt/robocode-s/read/1/r.jar")
            );
        }
    }

    #[test]
    fn container_paths_join_with_slashes() {
        // `PathBuf::join` on Windows puts a backslash after a path without a closing
        // separator; these end with one.
        assert_eq!(
            Path::new(HOME).join("tmp").to_str(),
            Some("/home/robot/tmp")
        );
        assert_eq!(
            Path::new(LIB).join("*").to_str(),
            Some("/opt/robocode-s/lib/*")
        );
    }

    #[test]
    fn podman_runs_its_image_as_the_user_without_root() {
        let a = engine_args(Engine::Podman, &User::KeepId, &["/h/lib"], "/h/home");
        assert!(has(&a, ["--userns", "keep-id"]), "{a:?}");
        assert!(!a.contains(&"--user".to_string()), "{a:?}");
        assert!(a.ends_with(&[PODMAN_IMAGE.to_string(), "java".to_string()]));
        // The same image, fully named and by digest alone.
        assert_eq!(PODMAN_IMAGE.rsplit('@').next(), IMAGE.rsplit('@').next());
        assert!(PODMAN_IMAGE.starts_with("docker.io/library/eclipse-temurin@sha256:"));
        // With root it is Docker's user.
        let a = engine_args(Engine::Podman, &User::Id("1000:1000".into()), &[], "/h");
        assert!(has(&a, ["--user", "1000:1000"]) && !a.contains(&"--userns".to_string()));
    }

    #[test]
    fn docker_is_refused_with_the_reason() {
        let docker = Engine::Docker;
        assert_eq!(
            refusal(docker, true, "linux\n", ""),
            Ok(Found { rootless: false })
        );
        assert_eq!(
            refusal(docker, true, "windows\n", ""),
            Err("docker runs windows containers; the robot host needs Linux ones".into())
        );
        let down = refusal(
            docker,
            false,
            "",
            "failed to connect to the docker API at unix:///var/run/docker.sock\nmore",
        )
        .unwrap_err();
        assert_eq!(
            down,
            "docker isn't running: failed to connect to the docker API at unix:///var/run/docker.sock"
        );
    }

    #[test]
    fn podman_says_whether_it_runs_without_root() {
        let podman = Engine::Podman;
        assert_eq!(
            refusal(podman, true, "linux true\n", ""),
            Ok(Found { rootless: true })
        );
        assert_eq!(
            refusal(podman, true, "linux false\n", ""),
            Ok(Found { rootless: false })
        );
        let down = refusal(
            podman,
            false,
            "",
            "Error: unable to connect to Podman socket\n",
        );
        assert_eq!(
            down,
            Err("podman isn't running: Error: unable to connect to Podman socket".into())
        );
        assert_eq!(
            refusal(podman, false, "", ""),
            Err("podman isn't running: `podman info` failed".into())
        );
    }

    #[test]
    fn the_image_is_the_pinned_jre() {
        let version = crate::jre::RELEASE
            .strip_prefix("jdk-")
            .unwrap()
            .replace('+', "_");
        assert!(
            IMAGE.starts_with(&format!("eclipse-temurin:{version}-jre@sha256:")),
            "{IMAGE} for {}",
            crate::jre::RELEASE
        );
        assert_eq!(IMAGE.rsplit(':').next().unwrap().len(), 64);
    }

    /// A home and a `lib` folder in a new temporary folder.
    fn grants(tag: &str) -> Grants {
        let dir =
            std::env::temp_dir().join(format!("robocode-s-docker-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::create_dir_all(dir.join("home/tmp")).unwrap();
        Grants {
            runtime: PathBuf::new(),
            read: vec![dir.join("lib")],
            home: dir.join("home"),
            memory_mb: 256,
        }
    }

    /// `docker run` (`podman run`) as for a robot, but running `program` instead of `java`.
    fn container(engine: Engine, name: &str, grants: &Grants, program: &[&str]) -> Sandboxed {
        let user = user(engine, &grants.home).unwrap();
        let mut args = run_args(engine, name, &user, grants).unwrap();
        args.pop();
        args.extend(program.iter().map(OsString::from));
        let mut command = engine.command();
        command.args(args);
        Sandboxed::container(command, engine, name.into())
    }

    /// What `program` prints in a container, and whether it succeeded.
    fn run(engine: Engine, grants: &Grants, program: &[&str]) -> (bool, String) {
        let name = format!(
            "robocode-s-test-{}",
            &robocode_s_server::random_token()[..8]
        );
        let output = container(engine, &name, grants, program).output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        (output.status.success(), text)
    }

    fn exists(engine: Engine, name: &str) -> bool {
        let output = engine
            .command()
            .args(["ps", "--all", "--quiet", "--filter"])
            .arg(format!("name={name}"))
            .output()
            .unwrap();
        !output.stdout.is_empty()
    }

    /// The engines that answer here, each prepared; the others are skipped, saying why.
    fn engines() -> Vec<Engine> {
        [Engine::Docker, Engine::Podman]
            .into_iter()
            .filter(|&engine| match unavailable(engine) {
                None => {
                    prepare(engine).unwrap();
                    true
                }
                Some(why) => {
                    eprintln!("skipped: {why}");
                    false
                }
            })
            .collect()
    }

    #[test]
    #[ignore = "needs Docker or Podman"]
    fn a_container_is_what_the_arguments_say() {
        for engine in engines() {
            container_is_what_the_arguments_say(engine);
        }
    }

    fn container_is_what_the_arguments_say(engine: Engine) {
        let g = grants(&format!("props-{}", engine.name()));
        let run = |program: &[&str]| run(engine, &g, program);
        // The JRE inside is the pinned one.
        let (ok, text) = run(&["java", "-version"]);
        let release = crate::jre::RELEASE.strip_prefix("jdk-").unwrap();
        assert!(ok && text.contains(&format!("Temurin-{release}")), "{text}");
        // No network: the loopback and nothing else.
        let (ok, text) = run(&["ls", "/sys/class/net"]);
        assert!(ok && text.trim() == "lo", "{text}");
        // A read-only root and read-only mounts; /tmp is a (small) memory file system.
        for path in ["/x", "/opt/robocode-s/lib/x", "/usr/x"] {
            let (ok, text) = run(&["touch", path]);
            assert!(!ok && text.contains("ead-only"), "{path}: {text}");
        }
        assert!(run(&["touch", "/tmp/x"]).0);
        // No capabilities, no new ones.
        let (_, text) = run(&["grep", "-E", "CapEff|NoNewPrivs", "/proc/self/status"]);
        assert!(
            text.contains("CapEff:\t0000000000000000") && text.contains("NoNewPrivs:\t1"),
            "{text}"
        );
        // The home is writable, and what is written there is the user's.
        let (ok, text) = run(&["sh", "-c", "echo data > /home/robot/data.txt && id -u"]);
        assert!(ok, "{text}");
        let file = g.home.join("data.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let uid = std::fs::metadata(&file).unwrap().uid();
            let me = Command::new("id").arg("-u").output().unwrap().stdout;
            assert_eq!(uid.to_string(), String::from_utf8_lossy(&me).trim());
            assert_eq!(text.trim(), uid.to_string());
        }
        assert_eq!(std::fs::read_to_string(file).unwrap(), "data\n");
        // The environment is the one `docker run` names.
        let (_, text) = run(&["env"]);
        assert!(
            text.contains("HOME=/home/robot/") && !text.contains("USER"),
            "{text}"
        );
        std::fs::remove_dir_all(g.home.parent().unwrap()).unwrap();
    }

    #[test]
    #[ignore = "needs Docker or Podman"]
    fn a_container_is_gone_when_it_ends_and_when_it_is_killed() {
        for engine in engines() {
            container_is_gone_when_it_ends_and_when_it_is_killed(engine);
        }
    }

    fn container_is_gone_when_it_ends_and_when_it_is_killed(engine: Engine) {
        let g = grants(&format!("kill-{}", engine.name()));
        let exists = |name: &str| exists(engine, name);
        let container = |name: &str, program: &[&str]| container(engine, name, &g, program);
        let gone = |name: &str| {
            let start = Instant::now();
            while exists(name) {
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "{name} is still there"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        // At its end.
        let name = "robocode-s-test-ends";
        let mut child = container(name, &["true"]).spawn(false, false).unwrap();
        assert!(child.wait().unwrap().success());
        gone(name);
        // Killed while running: the `docker` program's death alone would leave it.
        let name = "robocode-s-test-killed";
        let mut child = container(name, &["sleep", "600"])
            .spawn(true, false)
            .unwrap();
        let start = Instant::now();
        while !exists(name) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{name} never started"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        gone(name);
        std::fs::remove_dir_all(g.home.parent().unwrap()).unwrap();
    }
}
