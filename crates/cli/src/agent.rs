// SPDX-License-Identifier: EPL-2.0

//! `robocode-s agent`: a runner agent that starts robots for battles elsewhere (ADR 0020).

use robocode_s_runner::agent::{AgentConfig, JvmLauncher, Launcher, run};
use robocode_s_runner::local::Environment;
use robocode_s_server::tls::Fingerprint;
use std::sync::atomic::AtomicBool;

pub const USAGE: &str = "\
usage: robocode-s agent --server HOST:PORT --token TOKEN [options]

A runner agent: connects to a battle run with `robocode-s battle --agents`,
starts the robots it is given in its sandbox, fetches their jars (cached by
hash) and relays their stdin and stdout to the battle. It connects again after
each battle and when the connection drops; Ctrl+C stops it.

  --server HOST:PORT     the battle's --agents address
  --token TOKEN          the battle's --agent-token ($ROBOCODES_AGENT_TOKEN)
  --fingerprint F        the battle's certificate, as it prints it
                         (sha256:AB:CD:...); without one, TLS needs a
                         CA-issued certificate, and a loopback server is
                         reached without TLS
  --slots N              robots at once (default: no limit)
  --sandbox KIND         what robots run in: os, bwrap, docker, none, as for
                         `robocode-s battle` (default os where there is one)
  --data DIR             data directory: JRE, jar cache, robot homes
                         ($ROBOCODES_DATA)
  --java PATH            Java executable ($ROBOCODES_JAVA; default: pinned JRE)
  --host-lib DIR         robot host lib directory ($ROBOCODES_HOST_LIB)
  --agent-jar PATH       determinism agent ($ROBOCODES_AGENT_JAR)

The battle's machine is trusted: the robots' own CPU measurements count as for
local robots (design §7.3).
";

#[derive(Debug, Clone)]
pub struct Options {
    pub server: String,
    pub token: String,
    pub pin: Option<Fingerprint>,
    pub slots: u32,
    pub env: Environment,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Self {
            server: String::new(),
            token: std::env::var("ROBOCODES_AGENT_TOKEN").unwrap_or_default(),
            pin: None,
            slots: 0,
            env: Environment::from_env(),
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            match arg.as_str() {
                "--server" => o.server = value()?,
                "--token" => o.token = value()?,
                "--fingerprint" => {
                    o.pin = Some(
                        value()?
                            .parse()
                            .map_err(|e| format!("--fingerprint: {e}"))?,
                    )
                }
                "--slots" => {
                    let v = value()?;
                    o.slots = v
                        .parse()
                        .map_err(|_| format!("--slots: not a valid number: {v}"))?
                }
                "--sandbox" => {
                    o.env.sandbox = value()?.parse().map_err(|e| format!("--sandbox: {e}"))?
                }
                "--data" => o.env.data = value()?.into(),
                "--java" => o.env.java = Some(value()?.into()),
                "--host-lib" => o.env.host_lib = value()?.into(),
                "--agent-jar" => o.env.agent_jar = value()?.into(),
                a => return Err(format!("unknown option {a}")),
            }
        }
        if o.server.is_empty() {
            return Err("--server is required".into());
        }
        if o.token.is_empty() {
            return Err("--token is required (or $ROBOCODES_AGENT_TOKEN)".into());
        }
        Ok(o)
    }
}

pub fn run_agent(o: &Options) -> Result<(), String> {
    // Robots aren't known yet: only the robot host and the runtime are checked.
    let env = Environment {
        jars: Vec::new(),
        ..o.env.clone()
    };
    // Deterministic battles need the determinism agent; it is checked here once.
    let host = env.host(true)?;
    let dir = std::path::absolute(env.data.join("agent")).map_err(|e| e.to_string())?;
    let launcher = JvmLauncher::new(host, &dir)?;
    let config = AgentConfig {
        server: o.server.clone(),
        token: o.token.clone(),
        pin: o.pin,
        slots: o.slots,
        dir,
    };
    eprintln!("robocode-s agent: robots run on {}", launcher.info().0);
    let stop = AtomicBool::new(false);
    run(&config, &launcher, &stop, &mut |what| {
        eprintln!("robocode-s agent: {what}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn parses_the_agent_options() {
        let pin = format!("sha256:{}", ["AB"; 32].join(":"));
        let o = Options::parse(&args(&format!(
            "--server h:7655 --token t --fingerprint {pin} --slots 4 --sandbox none"
        )))
        .unwrap();
        assert_eq!(
            (o.server.as_str(), o.token.as_str(), o.slots),
            ("h:7655", "t", 4)
        );
        assert_eq!(o.pin, Some(Fingerprint([0xab; 32])));
        assert!(Options::parse(&args("--server h:1 --token t --fingerprint xyz")).is_err());
        assert!(Options::parse(&args("--token t")).is_err());
        if std::env::var_os("ROBOCODES_AGENT_TOKEN").is_none_or(|t| t.is_empty()) {
            assert!(Options::parse(&args("--server h:1")).is_err());
        }
    }
}
