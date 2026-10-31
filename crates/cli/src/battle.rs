// SPDX-License-Identifier: EPL-2.0

//! `robocode-s battle`: a headless battle with local robot processes.

use crate::results;
use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::housekeeping::keep_battles;
use robocode_s_runner::ide::{CLI_MARKER, marker, run_configuration};
use robocode_s_runner::local::{Environment, prepare_ide, random_seed, run_local_battle_with};
use robocode_s_runner::pool::Pool;
use robocode_s_runner::remote::run_remote_battle;
use robocode_s_server::agents::AgentListener;
use robocode_s_server::ide::{self, IdeListener};
use robocode_s_server::net::is_loopback;
use robocode_s_server::tls::Identity;
use robocode_s_server::{Observers, ServerConfig};
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub const USAGE: &str = "\
usage: robocode-s battle [options] <robot>...

Robots are class names as in classic Robocode, optionally with version
(sample.Fire, jk.mega.DrussGT 3.1.16); a team expands into its members.
`@ide` is a robot you debug from your IDE (see `robocode-s ide-config`): the
command prints the run configuration and waits for your IDE to start it. It takes
no time limits, so a breakpoint pauses the whole battle; the battle is marked as a
debug battle. One `@ide` per robot, after the others in the battle.

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
  --ide-port N           port robots from your IDE connect to (default 7878)
  --jar PATH             robot jar, or a class folder (an IDE's output, read in
                         place: robots there are named with classic's `*`, e.g.
                         `jd.MyRobot*`); repeatable (default: $ROBOCODES_ROBOT_JARS,
                         else the archive's robots, or the sample robots of a
                         built source tree)
  --memory MB            heap per robot JVM (default 512)
  --warm N               start N robot JVMs while the robots are looked up,
                         instead of afterwards (default: one per robot; 0 starts
                         them when the battle does)
  --data DIR             data directory: JRE, robots' data files, battle files
                         ($ROBOCODES_DATA; default: the user data folder from a
                         release archive, else target/robocodes-data)
  --fresh-data           robots start with no data files and keep none, as with
                         $ROBOCODES_FRESH_DATA (default: each robot keeps its
                         files from battle to battle, as in classic)
  --java PATH            Java executable ($ROBOCODES_JAVA; default: pinned JRE)
  --host-lib DIR         robot host lib directory ($ROBOCODES_HOST_LIB)
  --agent-jar PATH       determinism agent ($ROBOCODES_AGENT_JAR)
  --sandbox KIND         what robots run in (ADR 0019): os (Linux: seccomp, and
                         Landlock where the kernel has it; Windows: an
                         AppContainer and a Job Object), bwrap (bubblewrap,
                         if installed), docker (a container of the Temurin
                         JRE image, no network; needs Docker, pulls the image
                         the first time), podman (the same container in
                         Podman), none (a plain process); default os
                         where there is one, else none

Output:
  --json                 results as JSON instead of a table
  --record FILE          record the battle (replay with `robocode-s replay FILE`)
  --paint                ask every robot for its debug painting (stored by --record)

Runner agents (robots on other machines, `robocode-s agent`):
  --agents ADDR          listen for runner agents on ADDR (PORT, or HOST:PORT;
                         HOST defaults to 127.0.0.1) and run the robots on them,
                         round robin; this machine still describes them
  --agent-token TOKEN    agents must present TOKEN ($ROBOCODES_AGENT_TOKEN);
                         required with --agents
  --wait-agents N        wait for at least N agents (default 1)
  --agent-wait S         for up to S seconds (default 60)

Watching (the GUI's Watch Battle tab, Remote Server):
  --observe ADDR         let observers watch on ADDR (PORT, or HOST:PORT; HOST
                         defaults to 127.0.0.1); the battle starts once the first
                         one connected. Observers control the battle as the GUI
                         does (pause, speed, stop, kill, input).
  --observe-token TOKEN  observers must present TOKEN ($ROBOCODES_OBSERVE_TOKEN);
                         required unless ADDR is a loopback address

TLS (--agents and --observe): a listener that isn't on loopback uses TLS,
with a self-signed certificate made once in the data directory; its
fingerprint is printed, for clients to pin (--fingerprint).
  --tls                  TLS on loopback too
  --tls-cert FILE        a certificate chain (PEM) instead, e.g. from a CA,
  --tls-key FILE         and its key (PEM); clients without a fingerprint
                         check it against the Mozilla roots
";

/// The parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub robots: Vec<String>,
    /// `--ide-port`: where robots from an IDE connect (`@ide` in `robots`).
    pub ide_port: u16,
    pub rules: BattleRules,
    pub seed: Option<i64>,
    pub classic: bool,
    pub turn_timeout: Duration,
    /// Java, robot host, data directory and robot jars.
    pub env: Environment,
    pub memory_mb: u32,
    /// `--warm`: robot hosts to start ahead; `None`: one per robot.
    pub warm: Option<usize>,
    pub json: bool,
    pub record: Option<PathBuf>,
    pub paint: bool,
    /// `--observe`: where observers may watch (`host:port`).
    pub observe: Option<String>,
    /// The token observers present; empty: none (loopback only).
    pub observe_token: String,
    /// `--agents`: where runner agents connect (`host:port`).
    pub agents: Option<String>,
    pub agent_token: String,
    /// Agents to wait for, and for how long.
    pub wait_agents: usize,
    pub agent_wait: Duration,
    /// TLS on loopback too.
    pub tls: bool,
    /// A certificate chain and key (PEM) instead of the self-signed certificate.
    pub tls_cert: Option<(PathBuf, PathBuf)>,
}

impl Options {
    /// Defaults from the environment, else for a built source tree.
    fn defaults() -> Self {
        Self {
            robots: Vec::new(),
            ide_port: ide::DEFAULT_PORT,
            rules: BattleRules::default(),
            seed: None,
            classic: false,
            turn_timeout: ServerConfig::default().turn_timeout,
            env: Environment::from_env(),
            memory_mb: 512,
            warm: None,
            json: false,
            record: None,
            paint: false,
            observe: None,
            observe_token: std::env::var("ROBOCODES_OBSERVE_TOKEN").unwrap_or_default(),
            agents: None,
            agent_token: std::env::var("ROBOCODES_AGENT_TOKEN").unwrap_or_default(),
            wait_agents: 1,
            agent_wait: Duration::from_secs(60),
            tls: false,
            tls_cert: None,
        }
    }

    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Self::defaults();
        let mut jars = Vec::new();
        let (mut cert, mut key) = (None, None);
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
                "--ide-port" => o.ide_port = number(arg, value(arg)?)?,
                "--jar" => jars.push(PathBuf::from(value(arg)?)),
                "--memory" => o.memory_mb = number(arg, value(arg)?)?,
                "--warm" => o.warm = Some(number(arg, value(arg)?)?),
                "--data" => o.env.data = value(arg)?.into(),
                "--fresh-data" => o.env.fresh_data = true,
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
                "--agents" => o.agents = Some(observe_address(&value(arg)?)),
                "--agent-token" => o.agent_token = value(arg)?,
                "--wait-agents" => o.wait_agents = number(arg, value(arg)?)?,
                "--agent-wait" => o.agent_wait = Duration::from_secs(number(arg, value(arg)?)?),
                "--tls" => o.tls = true,
                "--tls-cert" => cert = Some(PathBuf::from(value(arg)?)),
                "--tls-key" => key = Some(PathBuf::from(value(arg)?)),
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
        o.tls_cert = match (cert, key) {
            (Some(c), Some(k)) => Some((c, k)),
            (None, None) => None,
            _ => return Err("--tls-cert and --tls-key go together".into()),
        };
        if o.agents.is_some() && o.agent_token.is_empty() {
            return Err("--agents needs a token for the agents: --agent-token (or \
                 $ROBOCODES_AGENT_TOKEN)"
                .into());
        }
        if o.wait_agents < 1 {
            return Err("--wait-agents must be at least 1".into());
        }
        if o.robots.is_empty() {
            return Err("no robots given".into());
        }
        if o.rules.num_rounds < 1 {
            return Err("--rounds must be at least 1".into());
        }
        if let Some(address) = &o.observe
            && o.observe_token.is_empty()
            && !is_loopback(address).map_err(|e| format!("--observe {e}"))?
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

/// The server's certificate for a listener on `address`: TLS off loopback, or with
/// `--tls` or `--tls-cert`; made once in the data directory unless given.
fn identity(
    o: &Options,
    address: &str,
    cache: &mut Option<Identity>,
) -> Result<Option<Identity>, String> {
    let loopback = is_loopback(address)?;
    if loopback && !o.tls && o.tls_cert.is_none() {
        return Ok(None);
    }
    if cache.is_none() {
        *cache = Some(match &o.tls_cert {
            Some((cert, key)) => Identity::from_pem(cert, key)?,
            None => Identity::load_or_create(&o.env.data)?,
        });
    }
    Ok(cache.clone())
}

/// How a client reaches a listener bound to `bound`.
fn reach(bound: std::net::SocketAddr) -> String {
    if bound.ip().is_unspecified() {
        format!("THIS-HOST:{}", bound.port())
    } else {
        bound.to_string()
    }
}

/// Listens for the robots from an IDE (one for each `@ide` in `robots`), prints how to
/// start them and waits for them; their entries in `robots` become the connected robots.
fn wait_for_ide(o: &Options, robots: &mut [String]) -> Result<IdeListener, String> {
    let count = robots.iter().filter(|r| *r == CLI_MARKER).count();
    let address = format!("127.0.0.1:{}", o.ide_port);
    let listener = IdeListener::bind(&address, ide::token(&o.env.data)?)
        .map_err(|e| format!("--ide-port {}: {e}", o.ide_port))?;
    eprintln!("{}", run_configuration(&o.env, o.ide_port)?.text());
    eprintln!(
        "Waiting for {count} robot{} from your IDE on {address} (Ctrl+C cancels)...",
        if count == 1 { "" } else { "s" }
    );
    // Nothing cancels but Ctrl+C, which ends the program.
    let cancel = AtomicBool::new(false);
    let here = loop {
        for k in 1..=count {
            if let Some(here) = listener.wait_for(k, &cancel) {
                eprintln!(
                    "{} connected ({k} of {count})",
                    here[k - 1].description.name
                );
            }
        }
        let here = listener.robots();
        if here.len() >= count {
            break here;
        }
    };
    let mut ids = here.iter().map(|r| r.id);
    for robot in robots.iter_mut().filter(|r| *r == CLI_MARKER) {
        *robot = marker(ids.next().unwrap_or_default());
    }
    Ok(listener)
}

pub fn run(o: &Options) -> Result<(), String> {
    let seed = o.seed.unwrap_or_else(random_seed);
    let mut certificate = None;
    // Bound first, so that a taken port fails before any robot starts.
    let agents = match &o.agents {
        Some(address) => {
            let tls = identity(o, address, &mut certificate)?;
            let listener = AgentListener::bind(address, tls, o.agent_token.clone())
                .map_err(|e| format!("--agents {address}: {e}"))?;
            let bound = listener.local_addr();
            match listener.fingerprint() {
                Some(f) => eprintln!(
                    "Listening for agents on {bound}, certificate {f} (robocode-s agent \
                     --server {} --fingerprint {f})",
                    reach(bound)
                ),
                None => eprintln!(
                    "Listening for agents on {bound} (robocode-s agent --server {})",
                    reach(bound)
                ),
            }
            Some(listener)
        }
        None => None,
    };
    let observers = match &o.observe {
        Some(address) => {
            let tls = identity(o, address, &mut certificate)?;
            let listener =
                TcpListener::bind(address).map_err(|e| format!("--observe {address}: {e}"))?;
            let bound = listener.local_addr().map_err(|e| e.to_string())?;
            let watch = reach(bound);
            match &tls {
                Some(t) => eprintln!(
                    "Waiting for an observer on {bound}, certificate {f} (robocode-s-gui \
                     --watch {watch} --fingerprint {f})",
                    f = t.fingerprint
                ),
                None => {
                    eprintln!("Waiting for an observer on {bound} (robocode-s-gui --watch {watch})")
                }
            }
            Some(Observers {
                listener,
                token: o.observe_token.clone(),
                tls,
                wait: true,
            })
        }
        None => None,
    };
    let mut robots = o.robots.clone();
    let from_ide = robots.iter().filter(|r| *r == CLI_MARKER).count();
    let ide = match from_ide {
        0 => None,
        _ if agents.is_some() => {
            return Err("@ide: robots from your IDE run on this machine, not on agents".into());
        }
        _ => Some(wait_for_ide(o, &mut robots)?),
    };
    let config = ServerConfig {
        rules: o.rules.clone(),
        seed,
        deterministic: !o.classic,
        turn_timeout: o.turn_timeout,
        record: o.record.clone(),
        ..Default::default()
    };
    // The robots' JVMs start while the robots are described. Robots on agents run there.
    let warm = o.warm.unwrap_or(robots.len() - from_ide);
    let pool = (warm > 0 && agents.is_none()).then(|| {
        let dir = o
            .env
            .data
            .join("pool")
            .join(format!("{}", std::process::id()));
        Arc::new(Pool::new(dir))
    });
    let (host, battle) = prepare_ide(
        &o.env,
        &robots,
        ide.as_ref(),
        config,
        o.memory_mb,
        pool.as_ref().map(|pool| (pool, warm)),
    )?;
    // One battle: the hosts started are all it needs.
    if let Some(pool) = &pool {
        pool.no_refill();
    }
    let debug = from_ide > 0;
    let statics: Vec<v1::RobotStatics> = battle.robots.iter().map(|r| r.statics.clone()).collect();
    let dir = battle.dir.clone();
    // The battle's folder goes when it is over; with this set it stays, with the robots'
    // output and error output in it.
    let keep = keep_battles();

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
    let on_message = |msg: &server_message::Msg| match msg {
        server_message::Msg::TurnSnapshot(s) if keep => {
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
    };
    let mut placements = Vec::new();
    let ran = match &agents {
        Some(agents) => {
            let remote = run_remote_battle(
                &battle,
                agents,
                (o.wait_agents, o.agent_wait),
                observers,
                controls,
                on_message,
            );
            remote.map(|remote| {
                placements = remote.agents;
                remote.local
            })
        }
        None => run_local_battle_with(&host, &battle, observers, controls, on_message),
    };
    // A local battle did so itself.
    battle.remove_dir();
    if let Some(ide) = &ide {
        ide.close();
    }
    let local = ran?;
    if progress {
        eprint!("\r\x1b[K");
    }
    let mut noisy = Vec::new();
    for (i, tail) in local.stderr.iter().enumerate() {
        if !tail.is_empty() {
            if keep {
                let _ = std::fs::write(dir.join(format!("stderr-{i}.txt")), tail);
            }
            noisy.push((&statics[i].name, tail));
        }
    }

    let rows = results::contestants(&statics, &local.outcome.results);
    // Robots whose code started threads of their own: their battles aren't reproducible.
    let threads: Vec<&str> = statics
        .iter()
        .zip(&local.outcome.threads)
        .filter(|(_, started)| **started)
        .map(|(s, _)| s.name.as_str())
        .collect();
    // Robots disabled by the hard limit: that depends on the machine (ADR 0025).
    let hard_limits: Vec<&str> = statics
        .iter()
        .zip(&local.outcome.hard_limits)
        .filter(|(_, hit)| **hit)
        .map(|(s, _)| s.name.as_str())
        .collect();
    if o.json {
        let out = serde_json::json!({
            "seed": seed,
            "rounds": local.outcome.rounds,
            "deterministic": !o.classic,
            "debug": debug,
            "engineVersion": robocode_s_engine::ENGINE_VERSION,
            "sandbox": host.sandbox.report(),
            "agents": placements,
            "robots": statics.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            "threads": threads,
            "hardLimit": hard_limits,
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
        if debug {
            println!(
                "Debug battle: robots from your IDE were not timed, so the results are not \
                 comparable with real battles.\n"
            );
        }
        for (robot, agent) in statics.iter().zip(&placements) {
            println!("{} ran on {agent}", robot.name);
        }
        if !placements.is_empty() {
            println!();
        }
        print!("{}", results::table(&rows));
        if !threads.is_empty() {
            println!(
                "\nStarted threads of their own, so this battle isn't reproducible: {}",
                threads.join(", ")
            );
        }
        if !hard_limits.is_empty() {
            println!(
                "\nDisabled by the hard limit (a turn took longer than {} s on this machine), so \
                 this battle may end differently elsewhere: {}",
                ServerConfig::default().hard_limit.as_secs(),
                hard_limits.join(", ")
            );
        }
    }
    if keep && !noisy.is_empty() {
        let names: Vec<&str> = noisy.iter().map(|(name, _)| name.as_str()).collect();
        eprintln!(
            "error output from {}: see {}",
            names.join(", "),
            dir.display()
        );
    } else {
        for (name, tail) in noisy {
            eprintln!("error output from {name}:\n{tail}");
        }
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
    fn robots_from_the_ide_are_marked_and_the_port_is_chosen() {
        let o = Options::parse(&args("sample.Walls,@ide,@ide --ide-port 9000")).unwrap();
        assert_eq!(o.robots, ["sample.Walls", "@ide", "@ide"]);
        assert_eq!(o.ide_port, 9000);
        let o = Options::parse(&args("@ide")).unwrap();
        assert_eq!(o.ide_port, ide::DEFAULT_PORT);
        assert!(Options::parse(&args("@ide --ide-port x")).is_err());
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
    fn agents_need_a_token() {
        if std::env::var_os("ROBOCODES_AGENT_TOKEN").is_none_or(|t| t.is_empty()) {
            let e = Options::parse(&args("sample.Fire --agents 7655")).unwrap_err();
            assert!(e.contains("--agent-token"), "{e}");
        }
        let o = Options::parse(&args(
            "sample.Fire --agents 0.0.0.0:7655 --agent-token t --wait-agents 2 --tls",
        ))
        .unwrap();
        assert_eq!(o.agents.as_deref(), Some("0.0.0.0:7655"));
        assert_eq!((o.wait_agents, o.tls), (2, true));
        assert!(Options::parse(&args("sample.Fire --tls-cert c.pem")).is_err());
        let o = Options::parse(&args("sample.Fire --tls-cert c.pem --tls-key k.pem")).unwrap();
        assert_eq!(o.tls_cert, Some(("c.pem".into(), "k.pem".into())));
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
    fn warm_hosts_default_to_the_robots() {
        assert_eq!(Options::parse(&args("a.B")).unwrap().warm, None);
        assert_eq!(Options::parse(&args("a.B --warm 0")).unwrap().warm, Some(0));
        assert_eq!(Options::parse(&args("a.B --warm 3")).unwrap().warm, Some(3));
        assert!(Options::parse(&args("a.B --warm x")).is_err());
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
