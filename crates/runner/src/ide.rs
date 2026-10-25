// SPDX-License-Identifier: EPL-2.0

//! Debugging a robot from an IDE (#17, design §5.2): the IDE starts the robot host, which
//! connects to the listener of `server::ide` and describes the robot; the battle is made
//! from the robots that are connected. This is the IDE side's configuration, and how a
//! battle names such robots.

use crate::local::{Described, Environment, user_data_dir};
use robocode_s_server::DETERMINISTIC_RULES;
use robocode_s_server::ide::{DEFAULT_PORT, IdeListener, check_description};

/// The robot host's main class, which an IDE runs.
pub const MAIN_CLASS: &str = "io.github.nhofsted.robocodes.compat.RobotHost";

/// What a command line writes for "a robot from my IDE" (`robocode-s battle sample.Walls,@ide`).
pub const CLI_MARKER: &str = "@ide";

/// How a battle's robot list names the robot from an IDE with this id ([`IdeListener`]).
pub fn marker(id: u64) -> String {
    format!("{CLI_MARKER}:{id}")
}

/// The id in a robot list's entry, if it is a robot from an IDE.
pub fn parse_marker(entry: &str) -> Option<u64> {
    entry.strip_prefix("@ide:")?.parse().ok()
}

/// A battle's robot list: the robots from an IDE (id and description), and the other names.
/// They must be connected to `ide` and follow the rules of the battle's mode.
#[allow(clippy::type_complexity)]
pub fn split(
    robots: &[String],
    ide: Option<&IdeListener>,
    deterministic: bool,
) -> Result<(Vec<(u64, Described)>, Vec<String>), String> {
    let mut from_ide = Vec::new();
    let mut names = Vec::new();
    for entry in robots {
        if !entry.starts_with(CLI_MARKER) {
            names.push(entry.clone());
            continue;
        }
        let id = parse_marker(entry).ok_or_else(|| {
            format!("{entry}: robots from an IDE are connected before the battle is made")
        })?;
        let robot = ide
            .and_then(|ide| ide.robots().into_iter().find(|r| r.id == id))
            .ok_or("a robot from your IDE isn't connected anymore")?;
        let d = robot.description;
        check_description(&d).map_err(|e| format!("{}: {e}", d.name))?;
        if deterministic && d.deterministic_rules != DETERMINISTIC_RULES {
            return Err(format!(
                "{}: its robot host follows deterministic rules {}, this program {DETERMINISTIC_RULES}: \
                 use the robot host of this version (robocode-s ide-config), or a classic battle",
                d.name, d.deterministic_rules
            ));
        }
        from_ide.push((
            id,
            Described {
                name: d.name,
                full_class_name: d.full_class_name,
                team_id: None,
                r#type: d.r#type.unwrap_or_default(),
            },
        ));
    }
    Ok((from_ide, names))
}

/// What to enter in an IDE to run a robot with its debugger: the robot host finds the
/// robots in the project's output itself (the class folders on its classpath), and its
/// token and home in the data folder.
#[derive(Debug, Clone, PartialEq)]
pub struct RunConfiguration {
    pub main_class: String,
    /// The robot host's jars: a folder, all of whose jars the classpath takes (`dir/*`).
    pub host_lib: String,
    /// `--ide`, and the server and the data folder where they aren't the robot host's
    /// defaults.
    pub arguments: Vec<String>,
    /// The determinism agent, for seeded random numbers (optional).
    pub agent: String,
}

/// The configuration for robots connecting to `127.0.0.1:port`. Makes the token the
/// first time.
pub fn run_configuration(env: &Environment, port: u16) -> Result<RunConfiguration, String> {
    let absolute = |p: &std::path::Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    robocode_s_server::ide::token(&env.data)?;
    let mut arguments = vec!["--ide".to_string()];
    if port != DEFAULT_PORT {
        arguments.extend(["--server".into(), format!("127.0.0.1:{port}")]);
    }
    // The robot host finds the user's data folder as this program does; another is named.
    let data = absolute(&env.data);
    if user_data_dir().map(|d| absolute(&d)) != Some(data.clone()) {
        arguments.extend(["--data".into(), data.display().to_string()]);
    }
    Ok(RunConfiguration {
        main_class: MAIN_CLASS.into(),
        host_lib: absolute(&env.host_lib).display().to_string(),
        arguments,
        agent: absolute(&env.agent_jar).display().to_string(),
    })
}

/// A string as JSON.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl RunConfiguration {
    /// A program argument or option for a command line or a text field: quoted if needed.
    fn quoted(arg: &str) -> String {
        if arg.contains(' ') {
            format!("\"{arg}\"")
        } else {
            arg.to_string()
        }
    }

    pub fn classpath(&self) -> String {
        let separator = if cfg!(windows) { '\\' } else { '/' };
        format!(
            "{}{separator}*",
            self.host_lib.trim_end_matches(['/', '\\'])
        )
    }

    pub fn arguments_line(&self) -> String {
        self.arguments
            .iter()
            .map(|a| Self::quoted(a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn agent_option(&self) -> String {
        format!("-javaagent:{}", self.agent)
    }

    /// A Visual Studio Code launch configuration (`launch.json`).
    pub fn vscode(&self) -> String {
        let q = json_string;
        let args = self.arguments.iter().map(|a| q(a)).collect::<Vec<_>>();
        format!(
            "{{\n  \"type\": \"java\",\n  \"request\": \"launch\",\n  \"name\": \"robocode-s robot\",\n  \
             \"mainClass\": {},\n  \"classPaths\": [\"$Auto\", {}],\n  \"args\": [{}]\n}}",
            q(&self.main_class),
            q(&self.classpath()),
            args.join(", "),
        )
    }

    /// The configuration as text to read and copy from.
    pub fn text(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "Run configuration for your IDE (Java 21 or newer; its Debug button starts it)\n\n",
        );
        let rows = [
            ("Main class", self.main_class.clone()),
            (
                "Classpath",
                format!(
                    "{}   (every jar in that folder, besides the project's own)",
                    self.classpath()
                ),
            ),
            ("Program arguments", self.arguments_line()),
        ];
        for (name, value) in rows {
            out.push_str(&format!("  {name:<18} {value}\n"));
        }
        out.push_str(&format!(
            "\nThe robot host offers the robots of your project's output (bin, target/classes, ...): \
             pick one in robocode-s. --robot NAME offers only that one.\n\
             Seeded random numbers (optional): add {} to the VM options and --seed N (the battle's seed) to the \
             program arguments.\n\n",
            self.agent_option()
        ));
        out.push_str(
            "IntelliJ IDEA: Run > Edit Configurations > + > Application; set the main class and the program \
             arguments; add the classpath folder to your module as a library (File > Project Structure > \
             Libraries > + > Java).\n\
             Eclipse: Run > Debug Configurations > Java Application; the main class; Classpath > User Entries > \
             Add External JARs (the jars of that folder); Arguments.\n\
             Visual Studio Code: add to launch.json (\"configurations\"):\n",
        );
        for line in self.vscode().lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("\nThis stays the same, so it is set up once.\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration() -> RunConfiguration {
        RunConfiguration {
            main_class: MAIN_CLASS.into(),
            host_lib: "/opt/robocode-s/lib/robot-host".into(),
            arguments: ["--ide", "--server", "127.0.0.1:7900", "--data", "/my data"]
                .map(String::from)
                .to_vec(),
            agent: "/opt/robocode-s/lib/agent/java-determinism.jar".into(),
        }
    }

    #[test]
    fn markers_round_trip() {
        assert_eq!(parse_marker(&marker(7)), Some(7));
        assert_eq!(parse_marker("@ide"), None);
        assert_eq!(parse_marker("sample.Walls"), None);
    }

    #[test]
    fn a_bare_marker_is_not_a_connected_robot() {
        let err = split(&["@ide".into()], None, false).unwrap_err();
        assert!(err.contains("connected"));
        let err = split(&[marker(3)], None, false).unwrap_err();
        assert!(err.contains("isn't connected"));
        let (ide, names) = split(&["sample.Walls".into()], None, true).unwrap();
        assert!(ide.is_empty());
        assert_eq!(names, ["sample.Walls"]);
    }

    #[test]
    fn the_text_has_every_part_to_copy() {
        let c = configuration();
        let text = c.text();
        for part in [
            MAIN_CLASS,
            "/opt/robocode-s/lib/robot-host/*",
            "--ide --server 127.0.0.1:7900 --data \"/my data\"",
            "-javaagent:/opt/robocode-s/lib/agent/java-determinism.jar",
            "launch.json",
        ] {
            assert!(text.contains(part), "{part} missing from\n{text}");
        }
        for gone in ["--add-opens", "ROBOCODES_IDE_TOKEN", "--jar", "--home"] {
            assert!(!text.contains(gone), "{gone} in\n{text}");
        }
    }

    #[test]
    fn the_vscode_configuration_is_json_with_escapes() {
        let mut c = configuration();
        c.host_lib = r"C:\robocode-s\lib\robot-host".into();
        let json = c.vscode();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["args"],
            serde_json::json!(["--ide", "--server", "127.0.0.1:7900", "--data", "/my data"])
        );
        assert!(json.contains("C:\\\\robocode-s"), "{json}");
    }

    #[test]
    fn the_defaults_need_only_ide() {
        let dir = std::env::temp_dir().join(format!("robocodes-ide-config-{}", std::process::id()));
        let mut env = Environment::from_env();
        env.data = dir.clone();
        let c = run_configuration(&env, DEFAULT_PORT).unwrap();
        assert_eq!(c.arguments[0], "--ide");
        assert!(!c.arguments.contains(&"--server".to_string()));
        // A data folder that isn't the user's is named.
        assert_eq!(
            c.arguments[1..],
            ["--data".to_string(), dir.display().to_string()]
        );
        if let Some(user) = user_data_dir() {
            let _ = std::fs::remove_dir_all(&dir);
            env.data = user;
            if env.data.join(robocode_s_server::ide::TOKEN_FILE).is_file() {
                assert_eq!(
                    run_configuration(&env, DEFAULT_PORT).unwrap().arguments,
                    ["--ide"]
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
