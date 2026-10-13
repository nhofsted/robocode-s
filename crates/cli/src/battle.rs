// SPDX-License-Identifier: EPL-2.0

//! `robocode-s battle`: a headless battle with local robot processes.

use crate::results;
use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::local::{
    LocalBattle, LocalRobot, battle_statics, describe, lookup_name, run_local_battle,
};
use robocode_s_runner::{JavaHost, jre};
use robocode_s_server::ServerConfig;
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const USAGE: &str = "\
usage: robocode-s battle [options] <robot>...

Robots are class names as in classic Robocode, optionally with version
(sample.Fire, jk.mega.DrussGT 3.1.16); a team expands into its members.

Battle:
  --rounds N             rounds (default 10)
  --seed N               battle seed (default: random, printed)
  --width N, --height N  battlefield size (default 800x600)
  --gun-cooling-rate X   (default 0.1)
  --inactivity-time N    turns (default 450)
  --sentry-border N      sentry border size (default 100)
  --hide-enemy-names
  --classic              classic timing: robots that are too slow skip turns,
                         instead of the deterministic mode (ADR 0005)
  --turn-timeout MS      classic mode: time per turn (default 50)

Robots and runtime:
  --jar PATH             robot jar; repeatable (default: $ROBOCODES_ROBOT_JARS,
                         else the sample robots of a built source tree)
  --memory MB            heap per robot JVM (default 512)
  --data DIR             data directory: JRE, battle files ($ROBOCODES_DATA,
                         default target/robocodes-data)
  --java PATH            Java executable ($ROBOCODES_JAVA; default: pinned JRE)
  --host-lib DIR         robot host lib directory ($ROBOCODES_HOST_LIB)
  --agent-jar PATH       determinism agent ($ROBOCODES_AGENT_JAR)

Output:
  --json                 results as JSON instead of a table
  --record FILE          record the battle (replay with `robocode-s replay FILE`)
  --paint                ask every robot for its debug painting (stored by --record)
";

/// The parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub robots: Vec<String>,
    pub rules: BattleRules,
    pub seed: Option<i64>,
    pub classic: bool,
    pub turn_timeout: Duration,
    pub jars: Vec<PathBuf>,
    pub memory_mb: u32,
    pub data: PathBuf,
    pub java: Option<PathBuf>,
    pub host_lib: PathBuf,
    pub agent_jar: PathBuf,
    pub json: bool,
    pub record: Option<PathBuf>,
    pub paint: bool,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Options {
    /// Defaults from the environment, else for a built source tree.
    fn defaults() -> Self {
        Self {
            robots: Vec::new(),
            rules: BattleRules::default(),
            seed: None,
            classic: false,
            turn_timeout: ServerConfig::default().turn_timeout,
            jars: std::env::var_os("ROBOCODES_ROBOT_JARS")
                .map(|v| {
                    std::env::split_paths(&v)
                        .filter(|p| !p.as_os_str().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            memory_mb: 512,
            data: env_path("ROBOCODES_DATA").unwrap_or_else(|| "target/robocodes-data".into()),
            java: env_path("ROBOCODES_JAVA"),
            host_lib: env_path("ROBOCODES_HOST_LIB")
                .unwrap_or_else(|| "clients/java-compat/build/install/robot-host/lib".into()),
            agent_jar: env_path("ROBOCODES_AGENT_JAR").unwrap_or_else(|| {
                "clients/java-determinism/build/libs/java-determinism-0.1.0-SNAPSHOT.jar".into()
            }),
            json: false,
            record: None,
            paint: false,
        }
    }

    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Self::defaults();
        let mut jars = Vec::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            fn number<T: std::str::FromStr>(name: &str, v: String) -> Result<T, String> {
                v.parse()
                    .map_err(|_| format!("{name}: not a valid number: {v}"))
            }
            match arg.as_str() {
                "--rounds" => o.rules.num_rounds = number(arg, value(arg)?)?,
                "--seed" => o.seed = Some(number(arg, value(arg)?)?),
                "--width" => o.rules.width = number(arg, value(arg)?)?,
                "--height" => o.rules.height = number(arg, value(arg)?)?,
                "--gun-cooling-rate" => o.rules.gun_cooling_rate = number(arg, value(arg)?)?,
                "--inactivity-time" => o.rules.inactivity_time = number(arg, value(arg)?)?,
                "--sentry-border" => o.rules.sentry_border_size = number(arg, value(arg)?)?,
                "--hide-enemy-names" => o.rules.hide_enemy_names = true,
                "--classic" => o.classic = true,
                "--turn-timeout" => {
                    o.turn_timeout = Duration::from_millis(number(arg, value(arg)?)?)
                }
                "--jar" => jars.push(PathBuf::from(value(arg)?)),
                "--memory" => o.memory_mb = number(arg, value(arg)?)?,
                "--data" => o.data = value(arg)?.into(),
                "--java" => o.java = Some(value(arg)?.into()),
                "--host-lib" => o.host_lib = value(arg)?.into(),
                "--agent-jar" => o.agent_jar = value(arg)?.into(),
                "--json" => o.json = true,
                "--record" => o.record = Some(value(arg)?.into()),
                "--paint" => o.paint = true,
                a if a.starts_with("--") => return Err(format!("unknown option {a}")),
                robot => o.robots.extend(
                    robot
                        .split(',')
                        .map(str::trim)
                        .filter(|r| !r.is_empty())
                        .map(String::from),
                ),
            }
        }
        if !jars.is_empty() {
            o.jars = jars;
        }
        if o.jars.is_empty() {
            o.jars
                .push("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar".into());
        }
        if o.robots.is_empty() {
            return Err("no robots given".into());
        }
        if o.rules.num_rounds < 1 {
            return Err("--rounds must be at least 1".into());
        }
        Ok(o)
    }
}

/// A seed for battles without one: different per run, printed so it can be repeated.
fn random_seed() -> i64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // splitmix64 finalizer over the time and process id.
    let mut z = (nanos as u64) ^ (u64::from(std::process::id()) << 32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) as i64
}

pub fn run(o: &Options) -> Result<(), String> {
    for jar in &o.jars {
        if !jar.is_file() {
            return Err(format!("robot jar not found: {}", jar.display()));
        }
    }
    if !o.host_lib.is_dir() {
        return Err(format!(
            "robot host not found in {} (./gradlew :clients:java-compat:installDist, or --host-lib)",
            o.host_lib.display()
        ));
    }
    if !o.classic && !o.agent_jar.is_file() {
        return Err(format!(
            "determinism agent not found: {} (./gradlew :clients:java-determinism:jar, or --agent-jar)",
            o.agent_jar.display()
        ));
    }
    let java = match &o.java {
        Some(java) => java.clone(),
        None => jre::java(&o.data).map_err(|e| format!("no Java runtime: {e}"))?,
    };
    let host = JavaHost {
        java,
        lib_dir: o.host_lib.clone(),
        agent_jar: o.agent_jar.clone(),
    };
    let seed = o.seed.unwrap_or_else(random_seed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let dir = std::path::absolute(o.data.join("battles"))
        .map_err(|e| e.to_string())?
        .join(format!("{stamp}-{}", std::process::id()));

    let described = describe(&host, &o.jars, &o.robots, &dir.join("describe"))?;
    let statics = battle_statics(&described);
    let battle = LocalBattle {
        config: ServerConfig {
            rules: o.rules.clone(),
            seed,
            deterministic: !o.classic,
            turn_timeout: o.turn_timeout,
            record: o
                .record
                .as_ref()
                .map(std::path::absolute)
                .transpose()
                .map_err(|e| e.to_string())?,
            ..Default::default()
        },
        robots: statics
            .iter()
            .map(|s| LocalRobot {
                lookup: lookup_name(&s.name).to_string(),
                statics: s.clone(),
            })
            .collect(),
        jars: o.jars.clone(),
        dir: dir.clone(),
        memory_mb: o.memory_mb,
    };

    let progress = !o.json && std::io::stderr().is_terminal();
    let rounds = o.rules.num_rounds;
    let mut outputs: Vec<Option<File>> = statics.iter().map(|_| None).collect();
    let controls = o.paint.then(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        for robot_index in 0..statics.len() as i32 {
            let _ = tx.send(v1::ObserverControl {
                control: Some(v1::observer_control::Control::Paint(v1::PaintToggle {
                    robot_index,
                    enabled: true,
                })),
            });
        }
        rx
    });
    let local = run_local_battle(&host, &battle, controls, |msg| match msg {
        server_message::Msg::TurnSnapshot(s) => {
            for (i, r) in s.robots.iter().enumerate() {
                if r.output.is_empty() {
                    continue;
                }
                let Some(slot) = outputs.get_mut(i) else {
                    continue;
                };
                if slot.is_none() {
                    *slot = File::create(dir.join(format!("output-{i}.txt"))).ok();
                }
                if let Some(f) = slot {
                    for line in r.output.lines() {
                        let _ = writeln!(f, "{} {}: {line}", s.round, s.turn);
                    }
                }
            }
        }
        server_message::Msg::RoundResult(r) if progress => {
            eprint!("\rround {}/{rounds}", r.round + 1);
        }
        _ => {}
    })?;
    if progress {
        eprint!("\r\x1b[K");
    }
    let mut noisy = Vec::new();
    for (i, tail) in local.stderr.iter().enumerate() {
        if !tail.is_empty() {
            let _ = std::fs::write(dir.join(format!("stderr-{i}.txt")), tail);
            noisy.push(statics[i].name.clone());
        }
    }

    let rows = results::contestants(&statics, &local.outcome.results);
    if o.json {
        let out = serde_json::json!({
            "seed": seed,
            "rounds": local.outcome.rounds,
            "deterministic": !o.classic,
            "robots": statics.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            "results": results::json(&rows),
        });
        println!("{out}");
    } else {
        let mode = if o.classic {
            "classic"
        } else {
            "deterministic"
        };
        println!(
            "{} rounds, seed {seed}, {mode} mode\n",
            local.outcome.rounds
        );
        print!("{}", results::table(&rows));
    }
    if !noisy.is_empty() {
        eprintln!(
            "error output from {}: see {}",
            noisy.join(", "),
            dir.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn parses_robots_and_rules() {
        let o = Options::parse(&args(
            "sample.Fire,sample.Walls sample.Crazy --rounds 35 --seed -7 --width 1000 --hide-enemy-names --json",
        ))
        .unwrap();
        assert_eq!(o.robots, ["sample.Fire", "sample.Walls", "sample.Crazy"]);
        assert_eq!(o.rules.num_rounds, 35);
        assert_eq!(o.rules.width, 1000);
        assert_eq!(o.rules.height, 600);
        assert!(o.rules.hide_enemy_names && o.json && !o.classic);
        assert_eq!(o.seed, Some(-7));
    }

    #[test]
    fn rejects_bad_command_lines() {
        assert!(Options::parse(&args("--rounds 3")).is_err());
        assert!(Options::parse(&args("sample.Fire --rounds")).is_err());
        assert!(Options::parse(&args("sample.Fire --rounds x")).is_err());
        assert!(Options::parse(&args("sample.Fire --rounds 0")).is_err());
        assert!(Options::parse(&args("sample.Fire --bogus")).is_err());
    }

    #[test]
    fn jars_on_the_command_line_replace_the_default() {
        let o = Options::parse(&args(
            "a.B --jar x.jar --jar y.jar --classic --turn-timeout 20",
        ))
        .unwrap();
        assert_eq!(o.jars, [PathBuf::from("x.jar"), PathBuf::from("y.jar")]);
        assert!(o.classic);
        assert_eq!(o.turn_timeout, Duration::from_millis(20));
    }
}
