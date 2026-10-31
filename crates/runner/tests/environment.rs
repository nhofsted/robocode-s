// SPDX-License-Identifier: EPL-2.0

//! A robot sees a fixed environment, never the user's (ADR 0025): in classic mode the fixed
//! process environment, in deterministic mode none at all and neutral system properties.
//! Needs Java and the built robot host, agent and robots (`./gradlew
//! :clients:java-compat:installDist :clients:java-determinism:jar :robots:samples:jar
//! :robots:testbots:jar`), so it is ignored by default: run it with `cargo test -p
//! robocode-s-runner --test environment -- --ignored`, best with a secret in the environment
//! (`AWS_SECRET_ACCESS_KEY=x ROBOCODES_RUMBLE_TOKEN=y cargo test ...`) to see it stay out.

use robocode_s_engine::BattleRules;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, prepare, run_local_battle};
use robocode_s_server::ServerConfig;
use std::path::{Path, PathBuf};

/// What testbots.Environ wrote: its `environ.txt`, wherever its data folder is.
fn seen(sandbox: Sandbox, deterministic: bool) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = Environment::from_env();
    for path in [&mut env.host_lib, &mut env.agent_jar, &mut env.data] {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    env.fresh_data = false;
    env.sandbox = sandbox;
    env.jars = env.jars.iter().map(|j| root.join(j)).collect();
    env.jars
        .push(root.join("robots/testbots/build/libs/testbots-0.1.0-SNAPSHOT.jar"));
    let data = env.robot_data().unwrap();
    let find = || files(&data, "environ.txt");
    for old in find() {
        std::fs::remove_file(old).unwrap();
    }
    let config = ServerConfig {
        rules: BattleRules {
            num_rounds: 1,
            ..Default::default()
        },
        seed: 5,
        deterministic,
        ..Default::default()
    };
    let robots = ["testbots.Environ".to_string(), "sample.Fire".to_string()];
    let (host, battle) = prepare(&env, &robots, config, 256).unwrap();
    run_local_battle(&host, &battle, None, |_| {}).unwrap();
    let found = find();
    assert_eq!(found.len(), 1, "{found:?}");
    std::fs::read_to_string(&found[0])
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn files(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files(&path, name));
        } else if path.file_name().is_some_and(|n| n == name) {
            found.push(path);
        }
    }
    found
}

#[test]
#[ignore = "needs Java and the built robots"]
fn robots_see_a_fixed_environment() {
    let all = [
        Sandbox::None,
        Sandbox::Os,
        Sandbox::Bwrap,
        Sandbox::Docker,
        Sandbox::Podman,
    ];
    for sandbox in all.into_iter().filter(|s| s.unavailable().is_none()) {
        eprintln!("{sandbox}");
        check(sandbox);
    }
}

fn check(sandbox: Sandbox) {
    let classic = seen(sandbox, false);
    let names: Vec<&str> = classic
        .iter()
        .filter_map(|l| l.strip_prefix("env "))
        .map(|l| l.split('=').next().unwrap())
        .collect();
    // Only the fixed set (the sandbox's own on Windows), none of the user's.
    #[cfg(not(windows))]
    if sandbox.is_container() {
        // The pinned image's own (and its entrypoint script's), and ours.
        let image = [
            "HOME",
            "HOSTNAME",
            "JAVA_HOME",
            "JAVA_VERSION",
            "JRE_CACERTS_PATH",
            "LANG",
            "LANGUAGE",
            "LC_ALL",
            "PATH",
            "PWD",
            "SHLVL",
            "TZ",
        ];
        assert_eq!(names, image, "{classic:?}");
        assert!(classic.iter().any(|l| l == "env HOSTNAME=robocode-s"));
    } else if sandbox == Sandbox::Bwrap {
        // bwrap sets `PWD`, the robot's home (older bwraps can't unset it).
        assert_eq!(names, ["HOME", "LANG", "PATH", "PWD", "TZ"], "{classic:?}");
        assert!(
            classic.iter().any(|l| l == "env LANG=C.UTF-8"),
            "{classic:?}"
        );
    } else {
        assert_eq!(names, ["HOME", "LANG", "PATH", "TZ"], "{classic:?}");
        assert!(
            classic.iter().any(|l| l == "env LANG=C.UTF-8"),
            "{classic:?}"
        );
    }
    #[cfg(windows)]
    assert!(
        names.iter().all(|n| {
            [
                "SystemRoot",
                "windir",
                "SystemDrive",
                "PROCESSOR_ARCHITECTURE",
                "LOCALAPPDATA",
                "PATH",
                "TEMP",
                "TMP",
            ]
            .iter()
            .any(|k| k.eq_ignore_ascii_case(n))
        }),
        "{classic:?}"
    );
    #[cfg(not(windows))]
    assert!(classic.iter().any(|l| l == "env TZ=UTC"), "{classic:?}");
    for property in [
        "property user.name=robocode-s",
        "property user.language=en",
        "property user.timezone=UTC",
        "property file.encoding=UTF-8",
    ] {
        assert!(
            classic.iter().any(|l| l == property),
            "{property}: {classic:?}"
        );
    }

    let deterministic = seen(sandbox, true);
    let props: Vec<&str> = deterministic.iter().map(String::as_str).collect();
    assert_eq!(
        props,
        [
            "property user.name=robocode-s",
            "property user.home=/home/robot",
            "property user.dir=/home/robot",
            "property os.name=Linux",
            "property java.home=/opt/java",
            "property line.separator=\\n",
            "property user.language=en",
            "property user.timezone=UTC",
            "property file.encoding=UTF-8",
        ],
        "{sandbox}: no environment, neutral properties"
    );
}
