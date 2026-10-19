// SPDX-License-Identifier: EPL-2.0

//! Local battles: the server in this process and one robot-host JVM per robot.
//!
//! Robots are first described by the robot host (`--describe`): classic's repository
//! expands teams and reports each robot's type. [`battle_statics`] then gives them their
//! unique battle names exactly as classic's `Battle.createPeers` does.

use crate::{JavaHost, MODULE_OPENINGS, RobotLaunch, Transport, launch};
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::{
    BattleOutcome, Observers, RobotEntry, RobotPipe, ServerConfig, random_token, run_battle_piped,
};
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// A robot as classic's repository describes it, before it gets its battle name.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    /// Unique full class name with version, e.g. `jk.mega.DrussGT 3.1.16`; also the name
    /// to look the robot up by.
    pub name: String,
    pub full_class_name: String,
    /// Classic's team id, e.g. `sampleteam.MyFirstTeam[   0]`; `None` without a team.
    pub team_id: Option<String>,
    pub r#type: v1::RobotType,
}

/// Asks the robot host what classic knows about `robots` (class names, optionally with
/// version; teams are expanded into their members), in battle order. For a `deterministic`
/// battle, the robot host must follow robocode-s's deterministic rules.
pub fn describe(
    host: &JavaHost,
    jars: &[PathBuf],
    robots: &[String],
    home: &Path,
    deterministic: bool,
) -> Result<Vec<Described>, String> {
    if robots.is_empty() {
        return Ok(Vec::new());
    }
    let text = run_describe(host, jars, robots, home)?;
    if deterministic {
        check_rules(&text, &host.lib_dir)?;
    }
    parse_description(&text)
}

/// Whether the robot host's `--describe` output (from `lib`) reports robocode-s's
/// deterministic rules. An outdated build counts robots' work differently: robots would
/// skip turns they don't skip in classic, or the other way around.
pub fn check_rules(text: &str, lib: &Path) -> Result<(), String> {
    let rules = text
        .lines()
        .find_map(|l| l.strip_prefix("rules\t"))
        .and_then(|v| v.trim().parse::<u32>().ok());
    if rules == Some(robocode_s_server::DETERMINISTIC_RULES) {
        return Ok(());
    }
    Err(format!(
        "the robot host in {} follows {} deterministic rules than this robocode-s ({}): \
         rebuild it (./gradlew :clients:java-compat:installDist)",
        lib.display(),
        match rules {
            Some(r) if r > robocode_s_server::DETERMINISTIC_RULES => format!("newer (version {r})"),
            Some(r) => format!("older (version {r})"),
            None => "older".to_string(),
        },
        robocode_s_server::DETERMINISTIC_RULES
    ))
}

/// A robot or team found in the jars or class folders, with the details from its `.properties` (empty
/// when it has none).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listed {
    /// The name to select it by, with version (`jk.mega.DrussGT 3.1.16`).
    pub name: String,
    pub team: bool,
    pub author: String,
    pub version: String,
    pub description: String,
    pub webpage: String,
    /// The Robocode version it was built for (`robocode.version`, as Tank Royale shows a
    /// bot's game version).
    pub robocode_version: String,
    /// The programming language, as Tank Royale shows it: classic's robots are JVM
    /// classes, "Java" (Kotlin, Scala, ... compile to the same; classic doesn't record it).
    pub language: String,
    /// What it runs on: the robot host's JVM (`JVM 21.0.4`).
    pub platform: String,
    /// The jar or class folder it is in, as given to the listing (`None` if the robot host
    /// didn't say).
    pub source: Option<PathBuf>,
}

/// Every robot and team in `jars` (jars and class folders), as classic's repository
/// finds them. A robot in a class folder is named with classic's `*` for a development
/// version (`mypackage.MyRobot*`), except classic's `sample.*` and `tested.*`.
///
/// Jars listed before come from a cache beside `dir` ([`listing`]), so only new and
/// changed jars and the class folders start classic's repository. A robot that is in two
/// jars is listed once.
pub fn list_robots(host: &JavaHost, jars: &[PathBuf], dir: &Path) -> Result<Vec<Listed>, String> {
    let mut robots = parse_list(&listing(host, jars, dir, true)?);
    robots.sort_by(|a, b| a.name.cmp(&b.name));
    robots.dedup_by(|a, b| a.name == b.name);
    Ok(robots)
}

/// The robot host's listing (`platform` and `list` lines) of the jars in `jars`, and of
/// its class folders with `folders`. A jar's lines are cached in `<dir>.cache.tsv` under
/// its size and last change, and all of them under the robot host's files, which make
/// the lines: a listing starts classic's repository only for jars that are new or
/// changed since, and for class folders, which change as the IDE compiles.
///
/// That listing gets a home of its own below `dir` (`<pid>-<n>`), removed afterwards:
/// the robot host empties its home's `robots` folder first, so listings sharing one
/// (two GUIs at once) would break each other. Homes a crash left behind are removed
/// once they are [`STALE_LISTING`] old.
fn listing(host: &JavaHost, jars: &[PathBuf], dir: &Path, folders: bool) -> Result<String, String> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let cache_file = {
        let mut name = dir.as_os_str().to_owned();
        name.push(".cache.tsv");
        PathBuf::from(name)
    };
    let fingerprint = host_fingerprint(&host.lib_dir);
    let mut cache = ListCache::read(&cache_file)
        .filter(|c| c.host == fingerprint)
        .unwrap_or_else(|| ListCache {
            host: fingerprint,
            ..ListCache::default()
        });
    let mut scan = Vec::new();
    let mut files = Vec::new();
    for jar in jars {
        if jar.is_dir() {
            if folders {
                scan.push(jar.clone());
            }
            continue;
        }
        let path = std::path::absolute(jar).map_err(|e| e.to_string())?;
        let stamp = file_stamp(&path);
        if stamp.is_none() || cache.jars.get(&path).map(|(s, _)| *s) != stamp {
            scan.push(jar.clone());
        }
        files.push(path);
    }
    let mut listed = Vec::new();
    if !scan.is_empty() {
        remove_stale(dir, STALE_LISTING);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let home = dir.join(format!("{}-{n}", std::process::id()));
        // A reused process id's leftovers.
        let _ = std::fs::remove_dir_all(&home);
        let output = run_describe(host, &scan, &[], &home);
        let _ = std::fs::remove_dir_all(&home);
        let output = output?;
        if let Some(platform) = output.lines().find_map(|l| l.strip_prefix("platform\t")) {
            cache.platform = platform.to_string();
        }
        let mut by_source: HashMap<&str, Vec<String>> = HashMap::new();
        for line in output.lines().filter(|l| l.starts_with("list\t")) {
            let source = line.rsplit('\t').next().unwrap_or("");
            by_source.entry(source).or_default().push(line.to_string());
        }
        for jar in &scan {
            let path = std::path::absolute(jar).map_err(|e| e.to_string())?;
            let lines = by_source
                .remove(path.to_string_lossy().as_ref())
                .unwrap_or_default();
            match file_stamp(&path) {
                Some(stamp) => {
                    cache.jars.insert(path, (stamp, lines));
                }
                // A class folder's robots, or a jar that just went away: listed, not cached.
                None => listed.extend(lines),
            }
        }
        // Robots without a source (an older robot host's listing) aren't cached either.
        listed.extend(by_source.into_values().flatten());
        cache.write(&cache_file);
    }
    let mut text = format!("platform\t{}\n", cache.platform);
    for path in &files {
        if let Some((_, lines)) = cache.jars.get(path) {
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    for line in listed {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

/// A jar's size and last change (nanoseconds since 1970): whenever it is replaced or
/// rebuilt, one of them changes.
type Stamp = (u64, u128);

fn file_stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let changed = metadata.modified().ok()?;
    let nanos = changed
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((metadata.len(), nanos))
}

/// The robot host's files with their stamps: a rebuilt robot host may list robots
/// differently, so it starts a new cache.
fn host_fingerprint(lib: &Path) -> String {
    let mut files: Vec<String> = std::fs::read_dir(lib)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let (size, changed) = file_stamp(&e.path())?;
            Some(format!(
                "{}:{size}:{changed}",
                e.file_name().to_string_lossy()
            ))
        })
        .collect();
    files.sort();
    files.join(" ")
}

/// The listing cache: per jar its stamp and its `list` lines.
#[derive(Debug, Default)]
struct ListCache {
    host: String,
    platform: String,
    jars: HashMap<PathBuf, (Stamp, Vec<String>)>,
}

impl ListCache {
    /// `host`, `platform`, then per jar a `jar` line (path, size, change) and its lines.
    fn read(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut lines = text.lines();
        let host = lines.next()?.strip_prefix("host\t")?.to_string();
        let platform = lines.next()?.strip_prefix("platform\t")?.to_string();
        let mut cache = Self {
            host,
            platform,
            jars: HashMap::new(),
        };
        let mut current = None;
        for line in lines {
            if let Some(jar) = line.strip_prefix("jar\t") {
                let mut f = jar.split('\t');
                let (path, size, changed) = (f.next()?, f.next()?, f.next()?);
                let stamp = (size.parse().ok()?, changed.parse().ok()?);
                cache.jars.insert(PathBuf::from(path), (stamp, Vec::new()));
                current = Some(PathBuf::from(path));
            } else if line.starts_with("list\t") {
                cache
                    .jars
                    .get_mut(current.as_ref()?)?
                    .1
                    .push(line.to_string());
            }
        }
        Some(cache)
    }

    /// Written to a file of its own first and then renamed, so two GUIs listing at once
    /// never read half a cache; the last one wins.
    fn write(&self, path: &Path) {
        let mut text = format!("host\t{}\nplatform\t{}\n", self.host, self.platform);
        let mut jars: Vec<_> = self.jars.iter().collect();
        jars.sort_by(|a, b| a.0.cmp(b.0));
        for (jar, ((size, changed), lines)) in jars {
            let Some(jar) = jar.to_str().filter(|j| !j.contains(['\t', '\n'])) else {
                continue;
            };
            text.push_str(&format!("jar\t{jar}\t{size}\t{changed}\n"));
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        let partial = path.with_extension(format!("{}.partial", std::process::id()));
        if std::fs::write(&partial, text).is_ok() && std::fs::rename(&partial, path).is_err() {
            let _ = std::fs::remove_file(&partial);
        }
    }
}

/// The jars and class folders a battle of `robots` (classic names, optionally with
/// version) needs: the jars the listing finds them in, and every class folder. Starting
/// classic's repository over every jar takes seconds per hundred jars, in the battle's
/// description and again in each robot's host. A team gets all of `jars`: its members
/// may be in other jars, and classic leaves out members it doesn't find. So does a
/// robot the listing doesn't find in a jar, unless there are class folders it may be in.
pub fn battle_jars(
    host: &JavaHost,
    jars: &[PathBuf],
    robots: &[String],
    dir: &Path,
) -> Vec<PathBuf> {
    match listing(host, jars, dir, false) {
        Ok(text) => jars_of(&parse_list(&text), jars, robots),
        Err(_) => jars.to_vec(),
    }
}

/// [`battle_jars`] from the listing of the jars in `jars`.
fn jars_of(listed: &[Listed], jars: &[PathBuf], robots: &[String]) -> Vec<PathBuf> {
    let folders: Vec<PathBuf> = jars.iter().filter(|j| j.is_dir()).cloned().collect();
    let mut needed: Vec<PathBuf> = Vec::new();
    for robot in robots {
        let found: Vec<&Listed> = listed
            .iter()
            .filter(|l| {
                l.name == *robot
                    || (!robot.contains(' ') && l.name.split(' ').next() == Some(robot))
            })
            .collect();
        if found.iter().any(|l| l.team || l.source.is_none())
            || (found.is_empty() && folders.is_empty())
        {
            return jars.to_vec();
        }
        for source in found.iter().filter_map(|l| l.source.as_ref()) {
            if !needed.contains(source) {
                needed.push(source.clone());
            }
        }
    }
    // In the order given: where classic finds a robot twice, the same one as before.
    jars.iter()
        .filter(|j| j.is_dir() || std::path::absolute(j).is_ok_and(|p| needed.contains(&p)))
        .cloned()
        .collect()
}

/// A listing takes seconds; one of its homes this old was left behind by a crash.
pub const STALE_LISTING: Duration = Duration::from_secs(60 * 60);

/// Removes what in `dir` was last changed more than `age` ago.
fn remove_stale(dir: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let stale = metadata
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|elapsed| elapsed > age);
        if stale {
            let _ = if metadata.is_dir() {
                std::fs::remove_dir_all(entry.path())
            } else {
                std::fs::remove_file(entry.path())
            };
        }
    }
}

/// Parses the robot host's `--describe` list: per robot its name, whether it is a team,
/// then its author, version, description, web page, Robocode version and the jar or class
/// folder it is in; and the platform they all run on.
fn parse_list(text: &str) -> Vec<Listed> {
    let platform = text
        .lines()
        .find_map(|l| l.strip_prefix("platform\t"))
        .unwrap_or("")
        .to_string();
    text.lines()
        .filter_map(|l| l.strip_prefix("list\t"))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?.to_string();
            let team = f.next()? == "1";
            let mut next = || f.next().unwrap_or("").to_string();
            Some(Listed {
                name,
                team,
                author: next(),
                version: next(),
                description: next(),
                webpage: next(),
                robocode_version: next(),
                source: Some(next()).filter(|s| !s.is_empty()).map(PathBuf::from),
                language: "Java".into(),
                platform: platform.clone(),
            })
        })
        .collect()
}

/// Runs the robot host's `--describe` and returns its output.
fn run_describe(
    host: &JavaHost,
    jars: &[PathBuf],
    robots: &[String],
    home: &Path,
) -> Result<String, String> {
    std::fs::create_dir_all(home).map_err(|e| format!("{}: {e}", home.display()))?;
    let lib = std::path::absolute(&host.lib_dir).map_err(|e| e.to_string())?;
    // Java runs in `home`, so a relative path to it must become absolute first.
    let java = crate::absolute_if_path(&host.java).map_err(|e| e.to_string())?;
    let mut command = Command::new(&java);
    crate::no_window(&mut command)
        .args(MODULE_OPENINGS)
        .arg("-cp")
        .arg(lib.join("*"))
        .arg("io.github.nhofsted.robocodes.compat.RobotHost")
        .arg("--describe")
        .arg("--home")
        .arg(std::path::absolute(home).map_err(|e| e.to_string())?);
    for jar in jars {
        command
            .arg("--jar")
            .arg(std::path::absolute(jar).map_err(|e| e.to_string())?);
    }
    for robot in robots {
        command.arg("--robot").arg(robot);
    }
    let output = command
        .current_dir(home)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("starting {}: {e}", java.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason: Vec<&str> = stderr.lines().filter(|l| !is_classic_noise(l)).collect();
        return Err(reason.join("\n"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parses the robot host's `--describe` output; other lines (classic's logging) are skipped.
pub fn parse_description(text: &str) -> Result<Vec<Described>, String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("robot\t"))
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            let [name, class, team, flags @ ..] = f.as_slice() else {
                return Err(format!("bad robot description: {line}"));
            };
            let flags: Vec<bool> = flags.iter().map(|f| *f == "1").collect();
            let [
                junior,
                advanced,
                team_robot,
                droid,
                sentry,
                interactive,
                paint,
            ] = flags.as_slice()
            else {
                return Err(format!("bad robot description: {line}"));
            };
            Ok(Described {
                name: name.to_string(),
                full_class_name: class.to_string(),
                team_id: (!team.is_empty()).then(|| team.to_string()),
                r#type: v1::RobotType {
                    junior: *junior,
                    advanced: *advanced,
                    team: *team_robot,
                    droid: *droid,
                    sentry: *sentry,
                    interactive: *interactive,
                    paint: *paint,
                },
            })
        })
        .collect()
}

/// The robots' statics with their battle names, as classic's `Battle.createPeers`: a
/// name that occurs more than once gets ` (1)`, ` (2)`, ...; a team is named after its
/// id without the `[   n]` suffix, plus ` (n)` for its position among the teams, and its
/// first member leads it.
pub fn battle_statics(robots: &[Described]) -> Vec<v1::RobotStatics> {
    let mut totals: HashMap<&str, usize> = HashMap::new();
    for r in robots {
        *totals.entry(&r.name).or_default() += 1;
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let names: Vec<String> = robots
        .iter()
        .map(|r| {
            let k = seen.entry(&r.name).or_default();
            *k += 1;
            if totals[r.name.as_str()] > 1 {
                format!("{} ({k})", r.name)
            } else {
                r.name.clone()
            }
        })
        .collect();
    let mut teams: Vec<&str> = Vec::new();
    let mut members: HashMap<&str, Vec<String>> = HashMap::new();
    for (r, name) in robots.iter().zip(&names) {
        if let Some(id) = &r.team_id {
            if !teams.contains(&id.as_str()) {
                teams.push(id);
            }
            members.entry(id).or_default().push(name.clone());
        }
    }
    robots
        .iter()
        .zip(&names)
        .map(|(r, name)| {
            let mut statics = v1::RobotStatics {
                name: name.clone(),
                full_class_name: r.full_class_name.clone(),
                r#type: Some(r.r#type),
                team_index: -1,
                ..Default::default()
            };
            if let Some(id) = &r.team_id {
                let index = teams.iter().position(|t| t == id).unwrap_or(0);
                // Java's `substring(0, length - 6)`; team ids are ASCII.
                let base = id.get(..id.len().saturating_sub(6)).unwrap_or(id);
                let team = &members[id.as_str()];
                statics.team_name = Some(format!("{base} ({})", index + 1));
                statics.team_index = index as i32;
                statics.team_leader = team.first() == Some(name);
                statics.team_members = team.clone();
            }
            statics
        })
        .collect()
}

/// Where the Java side, the robots and the battle files are.
#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    /// The `java` executable; `None` downloads the pinned JRE into `data`.
    pub java: Option<PathBuf>,
    /// The robot host's `lib` directory.
    pub host_lib: PathBuf,
    /// The determinism agent.
    pub agent_jar: PathBuf,
    /// The pinned JRE and battle files.
    pub data: PathBuf,
    /// Robot jars, and class folders (an IDE's output, e.g. `bin`): classic reads those
    /// in place as development paths, so a recompiled robot is new in the next battle.
    pub jars: Vec<PathBuf>,
}

impl Environment {
    /// From `ROBOCODES_JAVA`, `ROBOCODES_HOST_LIB`, `ROBOCODES_AGENT_JAR`, `ROBOCODES_DATA`
    /// and `ROBOCODES_ROBOT_JARS` (a path list of jars and class folders), else the paths
    /// of a built source tree.
    pub fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let jars: Vec<PathBuf> = std::env::var_os("ROBOCODES_ROBOT_JARS")
            .map(|v| {
                std::env::split_paths(&v)
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            java: var("ROBOCODES_JAVA"),
            host_lib: var("ROBOCODES_HOST_LIB")
                .unwrap_or_else(|| "clients/java-compat/build/install/robot-host/lib".into()),
            agent_jar: var("ROBOCODES_AGENT_JAR").unwrap_or_else(|| {
                "clients/java-determinism/build/libs/java-determinism-0.1.0-SNAPSHOT.jar".into()
            }),
            data: var("ROBOCODES_DATA").unwrap_or_else(|| "target/robocodes-data".into()),
            jars: if jars.is_empty() {
                vec!["robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar".into()]
            } else {
                jars
            },
        }
    }

    /// Checks the paths and finds Java (downloading the pinned JRE if needed).
    pub fn host(&self, deterministic: bool) -> Result<JavaHost, String> {
        for jar in &self.jars {
            if !jar.is_file() && !jar.is_dir() {
                return Err(format!("robot jar or folder not found: {}", jar.display()));
            }
        }
        if !self.host_lib.is_dir() {
            return Err(format!(
                "robot host not found in {} (./gradlew :clients:java-compat:installDist)",
                self.host_lib.display()
            ));
        }
        if deterministic && !self.agent_jar.is_file() {
            return Err(format!(
                "determinism agent not found: {} (./gradlew :clients:java-determinism:jar)",
                self.agent_jar.display()
            ));
        }
        let java = match &self.java {
            Some(java) => java.clone(),
            None => crate::jre::java(&self.data).map_err(|e| format!("no Java runtime: {e}"))?,
        };
        Ok(JavaHost {
            java,
            lib_dir: self.host_lib.clone(),
            agent_jar: self.agent_jar.clone(),
        })
    }
}

/// A seed for battles without one: different per run (report it, so the battle can be
/// fought again). Seeds are Java `long`s (classic's `RANDOMSEED`), so any `i64` is a seed,
/// negative ones too; the random ones are kept non-negative only so that they read alike.
pub fn random_seed() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // splitmix64 finalizer over the time and process id.
    let mut z = (nanos as u64) ^ (u64::from(std::process::id()) << 32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 1) as i64
}

/// Prepares a battle of `robots` (classic names; teams expand): describes and names them
/// and gives the battle its own directory under the data directory.
pub fn prepare(
    env: &Environment,
    robots: &[String],
    mut config: ServerConfig,
    memory_mb: u32,
) -> Result<(JavaHost, LocalBattle), String> {
    let host = env.host(config.deterministic)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let dir = std::path::absolute(env.data.join("battles"))
        .map_err(|e| e.to_string())?
        .join(format!("{stamp}-{}", std::process::id()));
    let mut jars = battle_jars(&host, &env.jars, robots, &env.data.join("list"));
    let mut described = describe(
        &host,
        &jars,
        robots,
        &dir.join("describe"),
        config.deterministic,
    );
    // A robot the cache didn't find where it is now: the description says what's wrong.
    if described.is_err() && jars != env.jars {
        jars = env.jars.clone();
        described = describe(
            &host,
            &jars,
            robots,
            &dir.join("describe"),
            config.deterministic,
        );
    }
    let described = described?;
    if let Some(path) = &config.record {
        config.record = Some(std::path::absolute(path).map_err(|e| e.to_string())?);
    }
    let battle = LocalBattle {
        config,
        robots: battle_statics(&described)
            .into_iter()
            .map(|s| LocalRobot {
                lookup: lookup_name(&s.name).to_string(),
                statics: s,
            })
            .collect(),
        jars,
        dir,
        memory_mb,
    };
    Ok((host, battle))
}

/// One robot of a local battle.
#[derive(Debug, Clone)]
pub struct LocalRobot {
    pub statics: v1::RobotStatics,
    /// The classic name the robot host looks the robot up by (its name without a
    /// duplicate suffix).
    pub lookup: String,
}

/// A battle to fight locally.
#[derive(Debug, Clone)]
pub struct LocalBattle {
    pub config: ServerConfig,
    pub robots: Vec<LocalRobot>,
    /// Jars and class folders containing the robots.
    pub jars: Vec<PathBuf>,
    /// This battle's directory: robot `i` runs in `robot-<i>` below it.
    pub dir: PathBuf,
    pub memory_mb: u32,
}

/// The end of a local battle.
#[derive(Debug)]
pub struct LocalOutcome {
    pub outcome: BattleOutcome,
    /// Per robot, the end of its error output, without classic's loading messages.
    pub stderr: Vec<String>,
}

/// Fights `battle`: starts the server, connects an observer that hands every message to
/// `observe` (on this thread), launches each robot and waits for the end of the battle.
/// `controls` are forwarded to the server as the observer's (pause, step, speed, paint).
pub fn run_local_battle(
    host: &JavaHost,
    battle: &LocalBattle,
    controls: Option<mpsc::Receiver<v1::ObserverControl>>,
    observe: impl FnMut(&server_message::Msg),
) -> Result<LocalOutcome, String> {
    run_local_battle_with(host, battle, None, controls, observe)
}

/// [`run_local_battle`], with observers from elsewhere on a listener of their own
/// (`robocode-s battle --observe`).
pub fn run_local_battle_with(
    host: &JavaHost,
    battle: &LocalBattle,
    observers: Option<Observers>,
    controls: Option<mpsc::Receiver<v1::ObserverControl>>,
    mut observe: impl FnMut(&server_message::Msg),
) -> Result<LocalOutcome, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let address = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    // The robots talk to the server over their stdin and stdout (ADR 0019); the listener
    // is for the battle's own observer. A fresh secret per battle for that: any local
    // process reaches it. It isn't recorded and doesn't affect the battle. The robots'
    // tokens are never used: their indexes are taken by their pipes.
    let entries: Vec<RobotEntry> = battle
        .robots
        .iter()
        .map(|r| RobotEntry {
            token: random_token(),
            statics: r.statics.clone(),
        })
        .collect();
    let mut processes = Vec::new();
    let mut pipes = Vec::new();
    for (i, robot) in battle.robots.iter().enumerate() {
        let launch_spec = RobotLaunch {
            transport: Transport::Stdio,
            robot: robot.lookup.clone(),
            jars: battle.jars.clone(),
            home: battle.dir.join(format!("robot-{i}")),
            seed: battle.config.seed,
            deterministic: battle.config.deterministic,
            memory_mb: battle.memory_mb,
        };
        let mut process = launch(host, &launch_spec)
            .map_err(|e| format!("launching {}: {e}", robot.statics.name))?;
        if let Some((reader, writer)) = process.take_pipes() {
            pipes.push(RobotPipe {
                robot: i,
                reader: Box::new(reader),
                writer: Box::new(writer),
            });
        }
        processes.push(process);
    }
    let config = ServerConfig {
        observer_token: random_token(),
        ..battle.config.clone()
    };
    let observer_token = config.observer_token.clone();
    let server =
        thread::spawn(move || run_battle_piped(listener, observers, &config, entries, pipes));

    let mut observer = TcpStream::connect(&address).map_err(|e| e.to_string())?;
    // Controls are small messages: sent at once, not held for the last one's ack.
    observer.set_nodelay(true).map_err(|e| e.to_string())?;
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Observer as i32,
            token: observer_token,
            ..Default::default()
        })),
    };
    write_message(&mut observer, &hello).map_err(|e| e.to_string())?;
    // Observer controls (pause, step, speed, paint) go to the server as they come.
    if let Some(controls) = controls {
        let mut writer = observer.try_clone().map_err(|e| e.to_string())?;
        thread::spawn(move || {
            for control in controls {
                let message = v1::ClientMessage {
                    msg: Some(client_message::Msg::ObserverControl(control)),
                };
                if let Err(e) = write_message(&mut writer, &message) {
                    robocode_s_server::log_perf(format_args!(
                        "window can't send controls anymore: {e}"
                    ));
                    break;
                }
            }
        });
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            match read_message::<v1::ServerMessage>(&mut observer) {
                Ok(Some(message)) => {
                    if let Some(msg) = message.msg
                        && tx.send(msg).is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    robocode_s_server::log_perf(format_args!(
                        "window's connection to the battle broke: {e}"
                    ));
                    break;
                }
            }
        }
    });

    // The battle ends with `BattleOver`; a connection that ends before it broke (the
    // server then stops the battle, see `ObserverLeft`).
    let mut over = false;
    for msg in rx {
        over |= matches!(msg, server_message::Msg::BattleOver(_));
        observe(&msg);
    }
    let outcome = server
        .join()
        .map_err(|_| "server panicked".to_string())?
        .map_err(|e| e.to_string())?;
    if !over {
        return Err("lost the connection to the battle".into());
    }
    let stderr = processes
        .iter_mut()
        .map(|p| {
            for _ in 0..100 {
                if p.try_wait().ok().flatten().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            p.stderr_tail()
                .lines()
                .filter(|l| !is_classic_noise(l))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    Ok(LocalOutcome { outcome, stderr })
}

/// Messages classic's robot side prints in every headless host: module loading, its
/// properties file, and the missing GUI module (no `robocode.ui` jar).
fn is_classic_noise(line: &str) -> bool {
    line.starts_with("Loaded ")
        || line.contains("robocode.properties")
        || line.starts_with("setVisible: No window manager was found.")
        || line.starts_with("Perhaps you are missing the robocode.ui")
        || line.trim().is_empty()
}

/// The classic name to look a robot up by: its unique name without a duplicate suffix.
pub fn lookup_name(name: &str) -> &str {
    match name.rfind(" (") {
        Some(i) if name.ends_with(')') => &name[..i],
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_listing_homes_are_removed() {
        let dir = std::env::temp_dir().join(format!("robocodes-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("running")).unwrap();
        let old = std::fs::File::create(dir.join("left-behind")).unwrap();
        let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        old.set_modified(two_hours_ago).unwrap();
        drop(old);
        remove_stale(&dir, STALE_LISTING);
        assert!(
            dir.join("running").exists(),
            "a listing still running stays"
        );
        assert!(!dir.join("left-behind").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listings_say_where_each_robot_is() {
        let text = "platform\tJVM 21\n\
                    list\ta.A 1.0\t0\tme\t1.0\t\t\t1.9\t/r/a.jar\n\
                    list\tb.B 2\t1\t\t\t\t\t\t\n";
        let listed = parse_list(text);
        assert_eq!(listed[0].source, Some(PathBuf::from("/r/a.jar")));
        assert_eq!(listed[0].robocode_version, "1.9");
        assert_eq!(listed[1].source, None, "an older robot host doesn't say");
        assert!(listed[1].team);
    }

    #[test]
    fn the_listing_cache_reads_what_it_wrote() {
        let path = std::env::temp_dir().join(format!("robocodes-cache-{}.tsv", std::process::id()));
        let mut cache = ListCache {
            host: "lib.jar:1:2".into(),
            platform: "JVM 21".into(),
            ..ListCache::default()
        };
        let line = "list\ta.A 1.0\t0\tme\t1.0\t\t\t1.9\t/r/a.jar".to_string();
        cache
            .jars
            .insert("/r/a.jar".into(), ((10, 20), vec![line.clone()]));
        cache.jars.insert("/r/empty.jar".into(), ((1, 2), vec![]));
        cache.write(&path);
        let read = ListCache::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            (read.host.as_str(), read.platform.as_str()),
            ("lib.jar:1:2", "JVM 21")
        );
        assert_eq!(read.jars[Path::new("/r/a.jar")], ((10, 20), vec![line]));
        assert_eq!(
            read.jars[Path::new("/r/empty.jar")],
            ((1, 2), vec![]),
            "jars without robots are cached too"
        );
    }

    #[test]
    fn a_battle_gets_only_its_robots_jars() {
        let robot = |name: &str, jar: &str, team: bool| Listed {
            name: name.into(),
            team,
            source: Some(PathBuf::from(jar)),
            ..Listed::default()
        };
        let listed = [
            robot("a.A 1.0", "/r/a.jar", false),
            robot("b.B 1.0", "/r/b1.jar", false),
            robot("b.B 1.0c", "/r/b2.jar", false),
            robot("t.Team 1", "/r/t.jar", true),
        ];
        let jars: Vec<PathBuf> = ["/r/b2.jar", "/r/a.jar", "/r/b1.jar", "/r/t.jar", "/r/c.jar"]
            .map(PathBuf::from)
            .to_vec();
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let pick = |n: &[&str]| jars_of(&listed, &jars, &names(n));
        assert_eq!(pick(&["a.A 1.0"]), [PathBuf::from("/r/a.jar")]);
        // Without a version, every version's jar, in the order given.
        assert_eq!(
            pick(&["b.B", "a.A 1.0"]),
            ["/r/b2.jar", "/r/a.jar", "/r/b1.jar"].map(PathBuf::from)
        );
        assert_eq!(
            pick(&["t.Team 1"]),
            jars,
            "a team's members may be anywhere"
        );
        assert_eq!(
            pick(&["a.A 1.0", "x.Unknown"]),
            jars,
            "nowhere else to look"
        );
    }

    #[test]
    fn random_seeds_are_non_negative() {
        // Any i64 is a seed, but the random ones read alike (#36: a screenshot showed a
        // negative one beside the usual positive ones).
        for _ in 0..1000 {
            assert!(random_seed() >= 0);
        }
    }

    #[test]
    fn parses_the_list_with_details() {
        let text = "noise\nplatform\tJVM 21.0.4\n\
                    list\tsample.Fire\t0\tMathew Nelson\t1.0\tShoots\thttp://x\t1.9.3.4\n\
                    list\tsampleteam.MyFirstTeam\t1\nlist\told.Robot\t0\t\t\t\t\n";
        let list = parse_list(text);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].author, "Mathew Nelson");
        assert_eq!(list[0].description, "Shoots");
        assert_eq!(list[0].webpage, "http://x");
        assert_eq!(list[0].robocode_version, "1.9.3.4");
        assert_eq!(
            (list[0].language.as_str(), list[0].platform.as_str()),
            ("Java", "JVM 21.0.4")
        );
        assert!(list[1].team && list[1].author.is_empty());
        assert!(list[2].robocode_version.is_empty());
        assert_eq!(list[2].name, "old.Robot");
    }

    fn robot(name: &str, team: Option<&str>) -> Described {
        Described {
            name: name.into(),
            full_class_name: name.split(' ').next().unwrap().into(),
            team_id: team.map(Into::into),
            r#type: v1::RobotType::default(),
        }
    }

    fn summary(statics: &[v1::RobotStatics]) -> Vec<(String, Option<String>, i32, bool)> {
        statics
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    s.team_name.clone(),
                    s.team_index,
                    s.team_leader,
                )
            })
            .collect()
    }

    fn s(
        name: &str,
        team: Option<&str>,
        index: i32,
        leader: bool,
    ) -> (String, Option<String>, i32, bool) {
        (name.into(), team.map(Into::into), index, leader)
    }

    #[test]
    fn parses_the_robot_host_description() {
        let text = "Loaded sample.Fire\nrobot\tsample.Fire\tsample.Fire\t\t0\t0\t0\t0\t0\t0\t0\n\
            robot\tsampleteam.MyFirstDroid\tsampleteam.MyFirstDroid\tsampleteam.MyFirstTeam[   0]\t0\t1\t1\t1\t0\t0\t0\n";
        let d = parse_description(text).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].team_id, None);
        assert!(!d[0].r#type.advanced);
        assert_eq!(
            d[1].team_id.as_deref(),
            Some("sampleteam.MyFirstTeam[   0]")
        );
        assert!(d[1].r#type.advanced && d[1].r#type.team && d[1].r#type.droid);
        assert!(parse_description("robot\tx\ty\t\t0\n").is_err());
    }

    #[test]
    fn deterministic_battles_need_the_same_rules() {
        let lib = Path::new("lib");
        let current = robocode_s_server::DETERMINISTIC_RULES;
        let robot = "robot\tsample.Fire\tsample.Fire\t\t0\t0\t0\t0\t0\t0\t0\n";
        assert!(check_rules(&format!("rules\t{current}\n{robot}"), lib).is_ok());
        // A robot host from before the rules had versions.
        let e = check_rules(robot, lib).unwrap_err();
        assert!(e.contains("older") && e.contains("installDist"), "{e}");
        let e = check_rules(&format!("rules\t{}\n{robot}", current + 1), lib).unwrap_err();
        assert!(e.contains("newer"), "{e}");
    }

    // Names from classic's recording of `team-vs-walls` (tools/difftest/record-samples.sh).
    #[test]
    fn names_a_team_and_its_duplicate_members_as_classic() {
        let team = Some("sampleteam.MyFirstTeam[   0]");
        let mut robots = vec![robot("sampleteam.MyFirstLeader", team)];
        robots.extend((0..4).map(|_| robot("sampleteam.MyFirstDroid", team)));
        robots.push(robot("sample.Walls", None));
        robots.push(robot("sample.SpinBot", None));
        let statics = battle_statics(&robots);
        let t = Some("sampleteam.MyFirstTeam (1)");
        assert_eq!(
            summary(&statics),
            vec![
                s("sampleteam.MyFirstLeader", t, 0, true),
                s("sampleteam.MyFirstDroid (1)", t, 0, false),
                s("sampleteam.MyFirstDroid (2)", t, 0, false),
                s("sampleteam.MyFirstDroid (3)", t, 0, false),
                s("sampleteam.MyFirstDroid (4)", t, 0, false),
                s("sample.Walls", None, -1, false),
                s("sample.SpinBot", None, -1, false),
            ]
        );
        assert_eq!(statics[0].team_members.len(), 5);
        assert_eq!(statics[5].team_members.len(), 0);
    }

    // Names from classic's recording of a RoboRumble team battle (corpus `36-teams`).
    #[test]
    fn numbers_teams_by_their_position() {
        let a = Some("mn.CombatTeam 3.25.0[   0]");
        let b = Some("kid.team.OmegaSquad .0.2[   1]");
        let mut robots: Vec<Described> = (0..5).map(|_| robot("mn.Combat 3.25.0", a)).collect();
        for m in ["Niner", "Darman", "Fi", "Atin"] {
            robots.push(robot(&format!("kid.team.{m} [.0.2]"), b));
        }
        let statics = battle_statics(&robots);
        assert_eq!(statics[0].name, "mn.Combat 3.25.0 (1)");
        assert_eq!(
            statics[0].team_name.as_deref(),
            Some("mn.CombatTeam 3.25.0 (1)")
        );
        assert!(statics[0].team_leader && !statics[1].team_leader);
        assert_eq!(statics[4].name, "mn.Combat 3.25.0 (5)");
        assert_eq!(statics[5].name, "kid.team.Niner [.0.2]");
        assert_eq!(
            statics[5].team_name.as_deref(),
            Some("kid.team.OmegaSquad .0.2 (2)")
        );
        assert_eq!(statics[5].team_index, 1);
        assert!(statics[5].team_leader && !statics[8].team_leader);
    }

    #[test]
    fn duplicates_outside_teams_are_numbered() {
        let statics = battle_statics(&[
            robot("sample.Fire", None),
            robot("sample.Walls", None),
            robot("sample.Fire", None),
        ]);
        let names: Vec<&str> = statics.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["sample.Fire (1)", "sample.Walls", "sample.Fire (2)"]
        );
        assert_eq!(lookup_name(names[2]), "sample.Fire");
        assert_eq!(
            lookup_name("kid.team.Niner [.0.2]"),
            "kid.team.Niner [.0.2]"
        );
    }

    // Two copies of one team: classic's ids tell them apart by selection position.
    #[test]
    fn two_copies_of_a_team_are_two_teams() {
        let robots = [
            robot(
                "sampleteam.MyFirstLeader",
                Some("sampleteam.MyFirstTeam[   0]"),
            ),
            robot(
                "sampleteam.MyFirstDroid",
                Some("sampleteam.MyFirstTeam[   0]"),
            ),
            robot("sample.Walls", None),
            robot(
                "sampleteam.MyFirstLeader",
                Some("sampleteam.MyFirstTeam[   2]"),
            ),
            robot(
                "sampleteam.MyFirstDroid",
                Some("sampleteam.MyFirstTeam[   2]"),
            ),
        ];
        assert_eq!(
            summary(&battle_statics(&robots)),
            vec![
                s(
                    "sampleteam.MyFirstLeader (1)",
                    Some("sampleteam.MyFirstTeam (1)"),
                    0,
                    true
                ),
                s(
                    "sampleteam.MyFirstDroid (1)",
                    Some("sampleteam.MyFirstTeam (1)"),
                    0,
                    false
                ),
                s("sample.Walls", None, -1, false),
                s(
                    "sampleteam.MyFirstLeader (2)",
                    Some("sampleteam.MyFirstTeam (2)"),
                    1,
                    true
                ),
                s(
                    "sampleteam.MyFirstDroid (2)",
                    Some("sampleteam.MyFirstTeam (2)"),
                    1,
                    false
                ),
            ]
        );
    }
}
