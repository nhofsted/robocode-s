// SPDX-License-Identifier: EPL-2.0

//! `robocode-s ide-config`: what to enter in an IDE to debug a robot (#17).

use robocode_s_runner::ide::run_configuration;
use robocode_s_runner::local::Environment;
use robocode_s_server::ide::DEFAULT_PORT;

pub const USAGE: &str = "\
usage: robocode-s ide-config [options]

Prints the run configuration for debugging a robot from your IDE: the IDE starts the
robot host with its debugger, and `robocode-s battle sample.Walls,@ide` (or the GUI's
\"From your IDE...\") waits for it. IntelliJ IDEA, Eclipse and Visual Studio Code.

  --ide-port N           port the robot connects to (default 7878)
  --json                 the parts as JSON
  --data DIR             data directory, where the token is kept ($ROBOCODES_DATA)
  --host-lib DIR         robot host lib directory ($ROBOCODES_HOST_LIB)
  --agent-jar PATH       determinism agent ($ROBOCODES_AGENT_JAR)
";

pub fn run(args: &[String]) -> Result<(), String> {
    let mut env = Environment::from_env();
    let (mut port, mut json) = (DEFAULT_PORT, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--ide-port" => {
                let v = value(arg)?;
                port = v
                    .parse()
                    .map_err(|_| format!("--ide-port: not a port: {v}"))?;
            }
            "--json" => json = true,
            "--data" => env.data = value(arg)?.into(),
            "--host-lib" => env.host_lib = value(arg)?.into(),
            "--agent-jar" => env.agent_jar = value(arg)?.into(),
            other => return Err(format!("unknown option {other}\n\n{USAGE}")),
        }
    }
    let c = run_configuration(&env, port)?;
    if json {
        let out = serde_json::json!({
            "mainClass": c.main_class,
            "classpath": c.classpath(),
            "programArguments": c.arguments,
            "javaAgent": c.agent_option(),
        });
        println!("{out}");
    } else {
        print!("{}", c.text());
    }
    Ok(())
}
