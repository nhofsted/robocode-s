// SPDX-License-Identifier: EPL-2.0

//! `robocode-s battle sample.Walls,@ide` with the real robot host started the way an IDE
//! does (#17). Needs Java and the built robot host and sample robots
//! (`./gradlew :clients:java-compat:installDist :clients:java-determinism:jar
//! :robots:samples:jar`), so it is ignored by default: `cargo test -p robocode-s-cli
//! --test ide -- --ignored` (`ROBOCODES_JAVA` names the JDK to use).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "needs Java and the built robots"]
fn a_robot_started_like_an_ide_takes_part_in_a_cli_battle() {
    let root = root();
    let dir = std::env::temp_dir().join(format!("robocodes-ide-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let data = dir.join("data");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let lib = root.join("clients/java-compat/build/install/robot-host/lib");
    let samples = root.join("robots/samples/build/libs/samples-0.1.0-SNAPSHOT.jar");
    let cli = Command::new(env!("CARGO_BIN_EXE_robocode-s"))
        .current_dir(&root)
        .args(["battle", "sample.Walls,@ide", "--rounds", "2", "--json"])
        .args(["--data".as_ref(), data.as_os_str()])
        .args(["--ide-port", &port.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The token appears when the command listens.
    let token_file = data.join("ide-token");
    let until = Instant::now() + Duration::from_secs(60);
    while !token_file.is_file() {
        assert!(Instant::now() < until, "the command never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    let token = std::fs::read_to_string(&token_file).unwrap();
    let java = std::env::var_os("ROBOCODES_JAVA").unwrap_or_else(|| "java".into());
    let host = Command::new(java)
        .args([
            "--add-opens=java.base/sun.net.www.protocol.jar=ALL-UNNAMED",
            "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
        ])
        .arg("-cp")
        .arg(lib.join("*"))
        .arg("io.github.nhofsted.robocodes.compat.RobotHost")
        .args(["--ide", "--server", &format!("127.0.0.1:{port}")])
        .args(["--robot", "sample.Fire", "--jar"])
        .arg(&samples)
        .arg("--home")
        .arg(data.join("ide-home"))
        .env("ROBOCODES_IDE_TOKEN", token.trim())
        .output()
        .unwrap();
    let out = cli.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        host.status.success(),
        "{}",
        String::from_utf8_lossy(&host.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["debug"], true);
    assert_eq!(json["rounds"], 2);
    assert_eq!(
        json["robots"],
        serde_json::json!(["sample.Walls", "sample.Fire"])
    );
    assert_eq!(json["results"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
