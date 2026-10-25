// SPDX-License-Identifier: EPL-2.0

use super::*;
use std::io::Write;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("robocodes-update-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn release(files: &[&str]) -> Release {
    Release {
        version: "9.0.0".into(),
        notes: "https://example/notes".into(),
        download: "https://example/dl".into(),
        files: files.iter().map(|f| f.to_string()).collect(),
    }
}

#[test]
fn compares_releases() {
    assert!(newer("0.2.10", "0.2.9"));
    assert!(newer("0.3.0", "0.2.99"));
    assert!(newer("1.0", "0.9.9"));
    assert!(!newer("0.2.1", "0.2.1"));
    assert!(!newer("0.2.1", "0.2.1-dev"));
    assert!(!newer("0.2.0", "0.2.1"));
    assert!(!newer("", "0.0.1"));
}

#[test]
fn reads_sha256sums() {
    let a = "a".repeat(64);
    let b = "B".repeat(64);
    let sums = format!("{a}  robocode-s-1-x.zip\n{b} *robocode-s-1-y.tar.gz\nshort  z\n");
    assert_eq!(sha256_of(&sums, "robocode-s-1-x.zip"), Some(a));
    assert_eq!(
        sha256_of(&sums, "robocode-s-1-y.tar.gz"),
        Some("b".repeat(64))
    );
    assert_eq!(sha256_of(&sums, "z"), None);
    assert_eq!(sha256_of(&sums, "robocode-s-1-x"), None);
}

#[test]
fn state_is_due_daily_and_offers_newer_unskipped_releases() {
    let mut state = State::default();
    assert!(state.due(now()));
    state.checked = 1_000_000;
    assert!(!state.due(1_000_000 + 3600));
    assert!(state.due(1_000_000 + CHECK_EVERY.as_secs()));
    assert!(state.due(999_000), "a clock set back");

    assert_eq!(state.offer(), None);
    state.latest = Some(release(&[]));
    assert_eq!(state.offer().map(|r| r.version.as_str()), Some("9.0.0"));
    state.skipped = Some("9.0.0".into());
    assert_eq!(state.offer(), None);
    state.skipped = None;
    state.latest.as_mut().unwrap().version = VERSION.into();
    assert_eq!(state.offer(), None);
}

#[test]
fn detects_installations() {
    let sys = temp("detect");
    let root = sys.join("home/u/robocode-s-0.2.1-linux-x86_64");
    let archive = Install::Archive { root: root.clone() };
    assert_eq!(detect_at(&root, "linux", &sys), archive);

    let usr = sys.join("usr/lib/robocode-s");
    assert_eq!(detect_at(&usr, "linux", &sys), Install::Package);
    write(&sys.join("etc/yum.repos.d/robocode-s.repo"), "");
    assert_eq!(
        detect_at(&usr, "linux", &sys),
        Install::Managed(Manager::Dnf)
    );
    write(&sys.join("etc/apt/sources.list.d/robocode-s.sources"), "");
    assert_eq!(
        detect_at(&usr, "linux", &sys),
        Install::Managed(Manager::Apt)
    );

    assert_eq!(detect_at(&root, "windows", &sys), archive);
    write(&root.join("uninstall.exe"), "");
    assert_eq!(
        detect_at(&root, "windows", &sys),
        Install::Setup { root: root.clone() }
    );

    let app = sys.join("Applications/robocode-s.app");
    let resources = app.join("Contents/Resources");
    assert_eq!(
        detect_at(&resources, "macos", &sys),
        Install::Bundle { app }
    );
    assert_eq!(detect_at(&root, "macos", &sys), archive);
    fs::create_dir_all(sys.join("opt/homebrew/Caskroom/robocode-s")).unwrap();
    assert_eq!(
        detect_at(&resources, "macos", &sys),
        Install::Managed(Manager::Homebrew)
    );
    let _ = fs::remove_dir_all(&sys);
}

#[test]
fn chooses_the_file_for_the_installation() {
    let arch = std::env::consts::ARCH;
    let os_archive = match std::env::consts::OS {
        "windows" => format!("windows-{arch}.zip"),
        "macos" => format!("macos-{arch}.tar.gz"),
        _ => format!("linux-{arch}.tar.gz"),
    };
    let file = format!("robocode-s-9.0.0-{os_archive}");
    let r = release(&[&file]);
    let archive = Install::Archive { root: "/x".into() };
    assert_eq!(archive.how(&r), How::Here { file });
    assert_eq!(archive.how(&release(&[])), How::Page);
    assert_eq!(
        Install::Managed(Manager::Apt).how(&r),
        How::Command(Manager::Apt.command())
    );
    assert_eq!(Install::Package.how(&r), How::Page);
    assert_eq!(
        Install::Bundle {
            app: "/a.app".into()
        }
        .how(&release(&[&format!(
            "robocode-s-9.0.0-macos-{arch}.app.zip"
        )])),
        How::Here {
            file: format!("robocode-s-9.0.0-macos-{arch}.app.zip")
        }
    );
}

/// An installation's files under `dir`, release `v`, with `extra` in `lib/robot-host`.
fn layout(dir: &Path, v: &str, extra: &str) {
    write(&dir.join("robocode-s"), v);
    write(&dir.join("README.txt"), v);
    write(&dir.join("lib/robot-host").join(extra), v);
    write(&dir.join("lib/agent/java-determinism.jar"), v);
    write(&dir.join(format!("robots/samples-{v}.jar")), v);
}

fn tar_gz(src: &Path, top: &str, out: &Path) {
    let gz = flate2::write::GzEncoder::new(fs::File::create(out).unwrap(), Default::default());
    let mut tar = tar::Builder::new(gz);
    tar.append_dir_all(top, src).unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}

fn zip_dir(src: &Path, top: &str, out: &Path) {
    let mut zip = zip::ZipWriter::new(fs::File::create(out).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    fn add(
        zip: &mut zip::ZipWriter<fs::File>,
        dir: &Path,
        prefix: &str,
        options: zip::write::SimpleFileOptions,
    ) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            if entry.path().is_dir() {
                zip.add_directory(&name, options).unwrap();
                add(zip, &entry.path(), &name, options);
            } else {
                zip.start_file(&name, options).unwrap();
                zip.write_all(&fs::read(entry.path()).unwrap()).unwrap();
            }
        }
    }
    zip.add_directory(top, options).unwrap();
    add(&mut zip, src, top, options);
    zip.finish().unwrap();
}

#[test]
fn replaces_an_archive_installation_keeping_the_users_robots() {
    let dir = temp("archive");
    let root = dir.join("robocode-s-0.2.1-linux-x86_64");
    layout(&root, "0.2.1", "old-dependency.jar");
    write(&root.join("robots/MyBot.jar"), "mine");
    write(&root.join("robots/classes/my/Bot.class"), "class");
    write(&root.join("notes.txt"), "the user's");
    let new = dir.join("new");
    layout(&new, "0.2.2", "new-dependency.jar");
    let archive = dir.join("robocode-s-0.2.2-linux-x86_64.tar.gz");
    tar_gz(&new, "robocode-s-0.2.2-linux-x86_64", &archive);

    replace(&root, &archive, Path::new("robots")).unwrap();

    assert_eq!(read(&root.join("robocode-s")), "0.2.2");
    assert_eq!(read(&root.join("README.txt")), "0.2.2");
    assert!(root.join("lib/robot-host/new-dependency.jar").is_file());
    assert!(!root.join("lib/robot-host/old-dependency.jar").exists());
    assert!(root.join("robots/samples-0.2.2.jar").is_file());
    assert!(!root.join("robots/samples-0.2.1.jar").exists());
    assert_eq!(read(&root.join("robots/MyBot.jar")), "mine");
    assert_eq!(read(&root.join("robots/classes/my/Bot.class")), "class");
    assert_eq!(read(&root.join("notes.txt")), "the user's");
    assert!(!root.join(NEW).exists() && !root.join(OLD).exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn replaces_a_bundle() {
    let dir = temp("bundle");
    let app = dir.join("robocode-s.app");
    layout(&app.join("Contents/Resources"), "0.2.1", "a.jar");
    write(&app.join("Contents/MacOS/robocode-s-gui"), "0.2.1");
    write(&app.join("Contents/Resources/robots/Mine.jar"), "mine");
    let new = dir.join("new");
    layout(&new.join("Contents/Resources"), "0.2.2", "b.jar");
    write(&new.join("Contents/MacOS/robocode-s-gui"), "0.2.2");
    let archive = dir.join("robocode-s-0.2.2-macos-aarch64.app.zip");
    zip_dir(&new, "robocode-s.app", &archive);

    replace(&app, &archive, Path::new("Contents/Resources/robots")).unwrap();

    assert_eq!(read(&app.join("Contents/MacOS/robocode-s-gui")), "0.2.2");
    assert!(!app.join("Contents/Resources/lib/robot-host/a.jar").exists());
    assert_eq!(
        read(&app.join("Contents/Resources/robots/Mine.jar")),
        "mine"
    );
    assert!(
        !app.join("Contents/Resources/robots/samples-0.2.1.jar")
            .exists()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn leaves_the_installation_alone_when_the_archive_is_wrong() {
    let dir = temp("wrong");
    let root = dir.join("root");
    layout(&root, "0.2.1", "a.jar");
    // Not a release: no lib folders.
    let new = dir.join("new");
    write(&new.join("robocode-s"), "evil");
    let archive = dir.join("x.tar.gz");
    tar_gz(&new, "top", &archive);
    let e = replace(&root, &archive, Path::new("robots")).unwrap_err();
    assert!(e.to_string().contains("not a robocode-s release"), "{e}");
    assert_eq!(read(&root.join("robocode-s")), "0.2.1");
    assert!(root.join("robots/samples-0.2.1.jar").is_file());
    assert!(!root.join(NEW).exists() && !root.join(OLD).exists());
    let _ = fs::remove_dir_all(&dir);
}

/// Serves `files` (path → body) over HTTP on 127.0.0.1 for `requests` requests.
fn serve(files: Vec<(&'static str, Vec<u8>)>, requests: usize) -> String {
    use std::io::{BufRead, BufReader};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap() <= 2 {
                    break;
                }
            }
            match files.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => {
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                }
                None => write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap(),
            }
        }
    });
    base
}

#[test]
fn downloads_and_checks_a_file() {
    use sha2::{Digest, Sha256};
    let body = b"the release".to_vec();
    let hash: String = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let sums = format!("{hash}  robocode-s-9.0.0-good.zip\n{hash}  robocode-s-9.0.0-bad.zip\n");
    let base = serve(
        vec![
            ("/SHA256SUMS", sums.into_bytes()),
            ("/robocode-s-9.0.0-good.zip", body.clone()),
            ("/robocode-s-9.0.0-bad.zip", b"tampered".to_vec()),
        ],
        5,
    );
    let r = Release {
        download: base,
        ..release(&[])
    };
    let data = temp("download");
    let path = download_to(&r, "robocode-s-9.0.0-good.zip", &data).unwrap();
    assert_eq!(fs::read(path).unwrap(), body);
    let e = download_to(&r, "robocode-s-9.0.0-bad.zip", &data).unwrap_err();
    assert!(e.to_string().contains("checksum mismatch"), "{e}");
    assert!(!data.join("robocode-s-9.0.0-bad.zip").exists());
    let e = download_to(&r, "robocode-s-9.0.0-missing.zip", &data).unwrap_err();
    assert!(
        e.to_string().contains("not in the release's SHA256SUMS"),
        "{e}"
    );
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn reads_release_json() {
    let json = r#"{"version": "0.2.2", "notes": "https://github.com/nhofsted/robocode-s/releases/tag/v0.2.2",
        "download": "https://github.com/nhofsted/robocode-s/releases/download/v0.2.2",
        "files": ["robocode-s-0.2.2-linux-x86_64.tar.gz"]}"#;
    let r: Release = serde_json::from_str(json).unwrap();
    assert_eq!(r.version, "0.2.2");
    assert_eq!(
        r.file("linux-x86_64.tar.gz").as_deref(),
        Some("robocode-s-0.2.2-linux-x86_64.tar.gz")
    );
    assert_eq!(r.file("linux-aarch64.tar.gz"), None);
}
