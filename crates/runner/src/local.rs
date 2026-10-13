// SPDX-License-Identifier: EPL-2.0

//! Local battles: the server in this process and one robot-host JVM per robot.
//!
//! Robots are first described by the robot host (`--describe`): classic's repository
//! expands teams and reports each robot's type. [`battle_statics`] then gives them their
//! unique battle names exactly as classic's `Battle.createPeers` does.

use crate::{JavaHost, MODULE_OPENINGS, RobotLaunch, launch};
use robocode_s_protocol::v1::{self, client_message, server_message};
use robocode_s_protocol::{PROTOCOL_VERSION, read_message, write_message};
use robocode_s_server::{BattleOutcome, RobotEntry, ServerConfig, run_battle};
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
/// version; teams are expanded into their members), in battle order.
pub fn describe(
    host: &JavaHost,
    jars: &[PathBuf],
    robots: &[String],
    home: &Path,
) -> Result<Vec<Described>, String> {
    std::fs::create_dir_all(home).map_err(|e| format!("{}: {e}", home.display()))?;
    let lib = std::path::absolute(&host.lib_dir).map_err(|e| e.to_string())?;
    // Java runs in `home`, so a relative path to it must become absolute first.
    let java = crate::absolute_if_path(&host.java).map_err(|e| e.to_string())?;
    let mut command = Command::new(&java);
    command
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
    parse_description(&String::from_utf8_lossy(&output.stdout))
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
    /// Robot jars.
    pub jars: Vec<PathBuf>,
}

impl Environment {
    /// From `ROBOCODES_JAVA`, `ROBOCODES_HOST_LIB`, `ROBOCODES_AGENT_JAR`, `ROBOCODES_DATA`
    /// and `ROBOCODES_ROBOT_JARS` (a path list), else the paths of a built source tree.
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
            if !jar.is_file() {
                return Err(format!("robot jar not found: {}", jar.display()));
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
/// fought again).
pub fn random_seed() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // splitmix64 finalizer over the time and process id.
    let mut z = (nanos as u64) ^ (u64::from(std::process::id()) << 32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) as i64
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
    let described = describe(&host, &env.jars, robots, &dir.join("describe"))?;
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
        jars: env.jars.clone(),
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
    /// Jars containing the robots.
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
    mut observe: impl FnMut(&server_message::Msg),
) -> Result<LocalOutcome, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let address = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .to_string();
    let entries: Vec<RobotEntry> = battle
        .robots
        .iter()
        .enumerate()
        .map(|(i, r)| RobotEntry {
            token: format!("robot-{i}"),
            statics: r.statics.clone(),
        })
        .collect();
    let config = battle.config.clone();
    let server = thread::spawn(move || run_battle(listener, &config, entries));

    let mut observer = TcpStream::connect(&address).map_err(|e| e.to_string())?;
    let hello = v1::ClientMessage {
        msg: Some(client_message::Msg::Hello(v1::Hello {
            version: Some(PROTOCOL_VERSION),
            role: v1::Role::Observer as i32,
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
                if write_message(&mut writer, &message).is_err() {
                    break;
                }
            }
        });
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(Some(message)) = read_message::<v1::ServerMessage>(&mut observer) {
            if let Some(msg) = message.msg
                && tx.send(msg).is_err()
            {
                break;
            }
        }
    });

    let mut processes = Vec::new();
    for (i, robot) in battle.robots.iter().enumerate() {
        let launch_spec = RobotLaunch {
            server: address.clone(),
            token: format!("robot-{i}"),
            robot: robot.lookup.clone(),
            jars: battle.jars.clone(),
            home: battle.dir.join(format!("robot-{i}")),
            seed: battle.config.seed,
            deterministic: battle.config.deterministic,
            memory_mb: battle.memory_mb,
        };
        processes.push(
            launch(host, &launch_spec)
                .map_err(|e| format!("launching {}: {e}", robot.statics.name))?,
        );
    }
    for msg in rx {
        observe(&msg);
    }
    let outcome = server
        .join()
        .map_err(|_| "server panicked".to_string())?
        .map_err(|e| e.to_string())?;
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
