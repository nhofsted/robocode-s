// SPDX-License-Identifier: EPL-2.0

//! `robocode-s battle`: a headless battle with local robot processes.

use crate::results;
use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::local::{Environment, prepare, random_seed, run_local_battle_with};
use robocode_s_server::Observers;
use robocode_s_server::ServerConfig;
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::net::{TcpListener, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "\
usage: robocode-s battle [options] <robot>...

Robots are class names as in classic Robocode, optionally with version
(sample.Fire, jk.mega.DrussGT 3.1.16); a team expands into its members.

Battle:
  --battle FILE          classic .battle file: its robots and rules (options
                         after it override its rules)
  --rounds N             rounds (default 10)
  --seed N               battle seed, a Java long (default: random, printed)
  --width N, --height N  battlefield size (default 800x600)
  --gun-cooling-rate X   (default 0.1)
  --inactivity-time N    turns (default 450)
  --sentry-border N      sentry border size (default 100)
  --hide-enemy-names
  --classic              classic timing: robots that are too slow skip turns,
                         instead of the deterministic mode (ADR 0005)
  --turn-timeout MS      classic mode: time per turn (default 50)

Robots and runtime:
  --jar PATH             robot jar, or a class folder (an IDE's output, read in
                         place: robots there are named with classic's `*`, e.g.
                         `my.Robot*`); repeatable (default: $ROBOCODES_ROBOT_JARS,
                         else the archive's robots, or the sample robots of a
                         built source tree)
  --memory MB            heap per robot JVM (default 512)
  --data DIR             data directory: JRE, battle files ($ROBOCODES_DATA;
                         default: the user data folder from a release archive,
                         else target/robocodes-data)
  --java PATH            Java executable ($ROBOCODES_JAVA; default: pinned JRE)
  --host-lib DIR         robot host lib directory ($ROBOCODES_HOST_LIB)
  --agent-jar PATH       determinism agent ($ROBOCODES_AGENT_JAR)
  --sandbox KIND         what robots run in (ADR 0019): os (Linux: seccomp, and
                         Landlock where the kernel has it; Windows: an
                         AppContainer and a Job Object), bwrap (bubblewrap,
                         if installed), docker (a container of the Temurin
                         JRE image, no network; needs Docker, pulls the image
                         the first time), none (a plain process); default os
                         where there is one, else none

Output:
  --json                 results as JSON instead of a table
  --record FILE          record the battle (replay with `robocode-s replay FILE`)
  --paint                ask every robot for its debug painting (stored by --record)

Watching (the GUI's Watch Battle tab, Remote Server):
  --observe ADDR         let observers watch on ADDR (PORT, or HOST:PORT; HOST
                         defaults to 127.0.0.1); the battle starts once the first
                         one connected. Observers control the battle as the GUI
                         does (pause, speed, stop, kill, input).
  --observe-token TOKEN  observers must present TOKEN ($ROBOCODES_OBSERVE_TOKEN);
                         required unless ADDR is a loopback address. The
                         connection is not encrypted: use a network you trust,
                         or an SSH tunnel to a loopback address
";

/// The parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub robots: Vec<String>,
    pub rules: BattleRules,
    pub seed: Option<i64>,
    pub classic: bool,
    pub turn_timeout: Duration,
    /// Java, robot host, data directory and robot jars.
    pub env: Environment,
    pub memory_mb: u32,
    pub json: bool,
    pub record: Option<PathBuf>,
    pub paint: bool,
    /// `--observe`: where observers may watch (`host:port`).
    pub observe: Option<String>,
    /// The token observers present; empty: none (loopback only).
    pub observe_token: String,
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
            env: Environment::from_env(),
            memory_mb: 512,
            json: false,
            record: None,
            paint: false,
            observe: None,
            observe_token: std::env::var("ROBOCODES_OBSERVE_TOKEN").unwrap_or_default(),
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
                "--battle" => {
                    let path = value(arg)?;
                    let text =
                        std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                    let file = robocode_s_server::battle_file::parse(&text, o.rules.clone())
                        .map_err(|e| format!("{path}: {e}"))?;
                    o.rules = file.rules;
                    o.robots.extend(file.robots);
                }
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
                "--data" => o.env.data = value(arg)?.into(),
                "--java" => o.env.java = Some(value(arg)?.into()),
                "--host-lib" => o.env.host_lib = value(arg)?.into(),
                "--agent-jar" => o.env.agent_jar = value(arg)?.into(),
                "--sandbox" => {
                    o.env.sandbox = value(arg)?.parse().map_err(|e| format!("--sandbox: {e}"))?
                }
                "--json" => o.json = true,
                "--record" => o.record = Some(value(arg)?.into()),
                "--paint" => o.paint = true,
                "--observe" => o.observe = Some(observe_address(&value(arg)?)),
                "--observe-token" => o.observe_token = value(arg)?,
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
            o.env.jars = jars;
        }
        if o.robots.is_empty() {
            return Err("no robots given".into());
        }
        if o.rules.num_rounds < 1 {
            return Err("--rounds must be at least 1".into());
        }
        if let Some(address) = &o.observe
            && o.observe_token.is_empty()
            && !is_loopback(address)?
        {
            return Err(format!(
                "--observe {address} is reachable from other machines: give observers a \
                 token with --observe-token (or $ROBOCODES_OBSERVE_TOKEN)"
            ));
        }
        Ok(o)
    }
}

/// `--observe`'s address: a port alone is on loopback.
fn observe_address(text: &str) -> String {
    match text.parse::<u16>() {
        Ok(port) => format!("127.0.0.1:{port}"),
        Err(_) => text.to_string(),
    }
}

/// Whether every address `address` stands for is a loopback one.
fn is_loopback(address: &str) -> Result<bool, String> {
    let addresses: Vec<_> = address
        .to_socket_addrs()
        .map_err(|e| format!("--observe {address}: {e}"))?
        .collect();
    Ok(!addresses.is_empty() && addresses.iter().all(|a| a.ip().is_loopback()))
}

pub fn run(o: &Options) -> Result<(), String> {
    let seed = o.seed.unwrap_or_else(random_seed);
    // Bound first, so that a taken port fails before any robot starts.
    let observers = match &o.observe {
        Some(address) => {
            let listener =
                TcpListener::bind(address).map_err(|e| format!("--observe {address}: {e}"))?;
            let bound = listener.local_addr().map_err(|e| e.to_string())?;
            let watch = if bound.ip().is_unspecified() {
                format!("THIS-HOST:{}", bound.port())
            } else {
                bound.to_string()
            };
            eprintln!("Waiting for an observer on {bound} (robocode-s-gui --watch {watch})");
            Some(Observers {
                listener,
                token: o.observe_token.clone(),
                wait: true,
            })
        }
        None => None,
    };
    let config = ServerConfig {
        rules: o.rules.clone(),
        seed,
        deterministic: !o.classic,
        turn_timeout: o.turn_timeout,
        record: o.record.clone(),
        ..Default::default()
    };
    let (host, battle) = prepare(&o.env, &o.robots, config, o.memory_mb)?;
    let statics: Vec<v1::RobotStatics> = battle.robots.iter().map(|r| r.statics.clone()).collect();
    let dir = battle.dir.clone();

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
    let local = run_local_battle_with(&host, &battle, observers, controls, |msg| match msg {
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
            "engineVersion": robocode_s_engine::ENGINE_VERSION,
            "sandbox": host.sandbox.report(),
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
            "{} rounds, seed {seed}, {mode} mode, engine {}, sandbox {}\n",
            local.outcome.rounds,
            robocode_s_engine::ENGINE_VERSION,
            host.sandbox.report()
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
    fn observers_need_a_token_off_loopback() {
        let o = Options::parse(&args("sample.Fire --observe 7654")).unwrap();
        assert_eq!(o.observe.as_deref(), Some("127.0.0.1:7654"));
        assert!(Options::parse(&args("sample.Fire --observe localhost:7654")).is_ok());
        let open = Options::parse(&args(
            "sample.Fire --observe 0.0.0.0:7654 --observe-token t0ken",
        ));
        assert_eq!(open.unwrap().observe_token, "t0ken");
        let open = Options::parse(&args("sample.Fire --observe 0.0.0.0:7654"));
        if std::env::var_os("ROBOCODES_OBSERVE_TOKEN").is_none_or(|t| t.is_empty()) {
            assert!(open.unwrap_err().contains("--observe-token"));
        }
    }

    #[test]
    fn reads_a_battle_file() {
        let path =
            std::env::temp_dir().join(format!("robocode-s-cli-{}.battle", std::process::id()));
        std::fs::write(
            &path,
            "robocode.battle.numRounds=35\nrobocode.battleField.width=1000\n\
             robocode.battle.selectedRobots=sample.Fire,sample.Walls\n",
        )
        .unwrap();
        let line = format!("--battle {} --rounds 3 sample.Crazy", path.display());
        let o = Options::parse(&args(&line)).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(o.robots, ["sample.Fire", "sample.Walls", "sample.Crazy"]);
        assert_eq!((o.rules.num_rounds, o.rules.width), (3, 1000));
        assert!(Options::parse(&args("--battle /no/such.battle")).is_err());
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
        assert_eq!(o.env.jars, [PathBuf::from("x.jar"), PathBuf::from("y.jar")]);
        assert!(o.classic);
        assert_eq!(o.turn_timeout, Duration::from_millis(20));
    }

    #[test]
    fn sandboxes_the_platform_lacks_are_refused() {
        use robocode_s_runner::Sandbox;
        let o = Options::parse(&args("a.B --sandbox none")).unwrap();
        assert_eq!(o.env.sandbox, Sandbox::None);
        // Docker is there or refused with the reason.
        match Options::parse(&args("a.B --sandbox docker")) {
            Ok(o) => assert_eq!(o.env.sandbox, Sandbox::Docker),
            Err(e) => assert!(e.starts_with("--sandbox: docker"), "{e}"),
        }
        let os = Options::parse(&args("a.B --sandbox os"));
        if cfg!(any(target_os = "linux", windows)) {
            assert_eq!(os.unwrap().env.sandbox, Sandbox::Os);
            assert_eq!(Options::defaults().env.sandbox, Sandbox::Os);
        } else {
            assert!(os.unwrap_err().ends_with("here robots run in: none"));
            assert_eq!(Options::defaults().env.sandbox, Sandbox::None);
        }
    }
}
