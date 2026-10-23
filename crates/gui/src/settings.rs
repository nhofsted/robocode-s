// SPDX-License-Identifier: EPL-2.0

//! The options (the gear's page), saved as JSON in the platform's config directory:
//! `$XDG_CONFIG_HOME/robocode-s/gui.json` (else `~/.config/…`) on Linux,
//! `~/Library/Application Support/robocode-s/gui.json` on macOS and
//! `%APPDATA%\robocode-s\gui.json` on Windows; `ROBOCODES_GUI_CONFIG` overrides it.

use robocode_s_runner::Sandbox;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The options. Missing fields take their defaults, so older files still load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folders scanned for robot jars. Not saved while it is the default (the sample
    /// robots of the installation), so that moving or upgrading the installation
    /// doesn't leave a stale folder in the file.
    #[serde(skip_serializing_if = "is_default_folders")]
    pub robot_folders: Vec<PathBuf>,
    /// What robots run in, by name (`--sandbox`, ADR 0019); see [`Settings::sandbox`].
    pub sandbox: String,
    /// Keep robot JVMs started between battles, so the next battle's robots start sooner
    /// (about 90 MB of memory each while they wait; see `runner::pool`).
    pub warm_hosts: bool,
    /// Listen for robots started by an IDE's debugger while the app is open, on this port
    /// of 127.0.0.1 (design §10, `crate::ide`).
    pub ide_listen: bool,
    pub ide_port: u16,
    /// Draw each robot's radar scan arc.
    pub scan_arcs: bool,
    /// Bullets in the robots' bullet colours rather than all in one.
    pub colored_bullets: bool,
    /// The rest of the view options (see `view::Style`).
    pub robot_names: bool,
    pub robot_energy: bool,
    pub explosions: bool,
    pub ground: bool,
    pub sentry_border: bool,
    pub rates: bool,
    /// Classic's sound effects (off by default, as in classic), and their volume (0 to 1).
    pub sound: bool,
    pub volume: f32,
    /// The defaults for new battles.
    pub classic: bool,
    pub rounds: i32,
    pub width: i32,
    pub height: i32,
    pub rules: Rules,
    /// The last battle watched (Watch Battle, Remote Server): its address. Its token is
    /// not saved.
    pub watch: String,
    /// Its certificate's fingerprint (`sha256:...`), if it was given; empty: none.
    pub watch_fingerprint: String,
    /// The energy and CPU graphs show the whole battle rather than the round.
    pub graphs_battle: bool,
    /// The most frames a second the animations draw; 0 for the display's refresh rate
    /// (see [`crate::pace`]).
    pub frame_rate: u32,
}

#[allow(clippy::ptr_arg)] // the signature serde's `skip_serializing_if` wants
fn is_default_folders(folders: &Vec<PathBuf>) -> bool {
    *folders == [robocode_s_runner::local::default_robot_dir()]
}

/// The battle rules besides the rounds and the battlefield size (classic's `BattleRules`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub gun_cooling_rate: f64,
    /// Turns without damage before the robots lose energy.
    pub inactivity_time: i64,
    pub hide_enemy_names: bool,
    pub sentry_border: i32,
}

impl Default for Rules {
    fn default() -> Self {
        let rules = robocode_s_engine::BattleRules::default();
        Self {
            gun_cooling_rate: rules.gun_cooling_rate,
            inactivity_time: rules.inactivity_time,
            hide_enemy_names: rules.hide_enemy_names,
            sentry_border: rules.sentry_border_size,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // The samples of the release archive or of a built source tree, as
            // `Environment::from_env` assumes.
            robot_folders: vec![robocode_s_runner::local::default_robot_dir()],
            sandbox: Sandbox::default().name().into(),
            warm_hosts: true,
            ide_listen: true,
            ide_port: robocode_s_server::ide::DEFAULT_PORT,
            scan_arcs: false,
            colored_bullets: true,
            robot_names: true,
            robot_energy: true,
            explosions: true,
            ground: true,
            sentry_border: true,
            rates: false,
            sound: false,
            volume: 0.7,
            classic: false,
            rounds: 10,
            width: 800,
            height: 600,
            rules: Rules::default(),
            watch: "localhost:7654".into(),
            watch_fingerprint: String::new(),
            graphs_battle: false,
            frame_rate: 0,
        }
    }
}

impl Settings {
    /// The settings file, if the platform has a config directory; `ROBOCODES_GUI_CONFIG`
    /// names another file.
    pub fn path() -> Option<PathBuf> {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        if let Some(file) = var("ROBOCODES_GUI_CONFIG") {
            return Some(file.into());
        }
        let dir = if cfg!(windows) {
            var("APPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            var("HOME").map(|h| Path::new(&h).join("Library/Application Support"))
        } else {
            var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| var("HOME").map(|h| Path::new(&h).join(".config")))
        }?;
        Some(dir.join("robocode-s").join("gui.json"))
    }

    /// The saved settings; the defaults when there are none. A file that can't be read is
    /// reported and left alone (it is overwritten on the next change).
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|e| {
                eprintln!("robocode-s-gui: ignoring {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let mut settings: Self = serde_json::from_str(text)?;
        settings.find_samples(&robocode_s_runner::local::default_robot_dir());
        Ok(settings)
    }

    /// Points a source tree's sample folder at `samples`, this program's: files written
    /// before the default folder was left out of them name it, and an installation has
    /// its samples elsewhere.
    fn find_samples(&mut self, samples: &Path) {
        let source = Path::new(robocode_s_runner::local::SOURCE_ROBOTS);
        for folder in &mut self.robot_folders {
            if folder == source {
                *folder = samples.to_path_buf();
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.robot_folders.retain(|f| seen.insert(f.clone()));
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("no config directory (HOME is not set)")?;
        let fail = |e: std::io::Error| format!("{}: {e}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Written aside and then moved over the file, so that a crash while writing never
        // leaves half a file (which would load as the defaults).
        let mut aside = path.clone().into_os_string();
        aside.push(".tmp");
        std::fs::write(&aside, text + "\n").map_err(fail)?;
        std::fs::rename(&aside, &path).map_err(fail)
    }

    /// The sandbox chosen, if this platform has it (a file from another one may name one
    /// it doesn't); else the platform's default.
    pub fn sandbox(&self) -> Sandbox {
        self.sandbox.parse().unwrap_or_default()
    }

    /// Where the robots are: per robot folder its `.jar` files, then the folder itself if
    /// it holds class files (an IDE's output, read in place as classic's development
    /// path); a folder entry may also name a jar. Then `extra` (from
    /// `ROBOCODES_ROBOT_JARS`). Each once, jars sorted.
    pub fn robot_paths(&self, extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for folder in &self.robot_folders {
            let contents = Contents::of(folder);
            paths.extend(contents.jars);
            if contents.classes {
                paths.push(folder.clone());
            }
        }
        paths.extend(extra.iter().cloned());
        let mut seen = std::collections::HashSet::new();
        paths.retain(|p| seen.insert(p.clone()));
        paths
    }
}

/// What a robot folder holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    /// The `.jar` files in it (not in its subfolders), sorted.
    pub jars: Vec<PathBuf>,
    /// Class files in it or below: a class folder.
    pub classes: bool,
}

impl Contents {
    /// What `folder` holds; nothing if it can't be read. A jar named as a folder holds
    /// itself.
    pub fn of(folder: &Path) -> Self {
        if is_jar(folder) && folder.is_file() {
            return Self {
                jars: vec![folder.to_path_buf()],
                classes: false,
            };
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            return Self::default();
        };
        let mut jars: Vec<PathBuf> = entries
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| is_jar(p) && p.is_file())
            .collect();
        jars.sort();
        Self {
            jars,
            classes: has_classes(folder),
        }
    }
}

/// Whether there is a `.class` file in `folder` or a few levels below (its packages),
/// looking at a bounded number of entries so that a large folder stays quick. Hidden
/// folders are skipped.
fn has_classes(folder: &Path) -> bool {
    const DEPTH: usize = 8;
    const ENTRIES: usize = 20_000;
    let mut seen = 0;
    let mut stack = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENTRIES {
                return false;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if depth < DEPTH && !hidden {
                    stack.push((path, depth + 1));
                }
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("class"))
            {
                return true;
            }
        }
    }
    false
}

fn is_jar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jar"))
}

/// `ROBOCODES_ROBOT_JARS`: extra robot jars and class folders (a path list), as for
/// `robocode-s battle`.
pub fn env_jars() -> Vec<PathBuf> {
    std::env::var_os("ROBOCODES_ROBOT_JARS")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s = Settings::parse(r#"{"scan_arcs": true, "rounds": 3}"#).unwrap();
        assert!(s.scan_arcs);
        assert_eq!(s.rounds, 3);
        assert_eq!(s.width, Settings::default().width);
        assert_eq!(s.robot_folders, Settings::default().robot_folders);
        assert_eq!(s.rules.inactivity_time, 450);
        assert_eq!(s.sandbox(), Sandbox::default());
    }

    #[test]
    fn the_default_robot_folder_is_not_saved() {
        let text = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!text.contains("robot_folders"), "{text}");
        let s = Settings::parse(&text).unwrap();
        assert_eq!(s.robot_folders, Settings::default().robot_folders);
        let changed = Settings {
            robot_folders: vec!["/mine".into()],
            ..Settings::default()
        };
        let text = serde_json::to_string(&changed).unwrap();
        assert!(text.contains("/mine"), "{text}");
    }

    #[test]
    fn a_source_trees_sample_folder_becomes_the_installations() {
        let mut s = Settings {
            robot_folders: vec![
                "robots/samples/build/libs".into(),
                "/mine".into(),
                "/install/robots".into(),
            ],
            ..Settings::default()
        };
        s.find_samples(Path::new("/install/robots"));
        assert_eq!(
            s.robot_folders,
            [PathBuf::from("/install/robots"), "/mine".into()]
        );
    }

    #[test]
    fn a_sandbox_this_platform_lacks_gives_the_default() {
        let s = Settings::parse(r#"{"sandbox": "chroot"}"#).unwrap();
        assert_eq!(s.sandbox(), Sandbox::default());
        let s = Settings::parse(r#"{"sandbox": "none"}"#).unwrap();
        assert_eq!(s.sandbox(), Sandbox::None);
    }

    #[test]
    fn round_trips() {
        let s = Settings {
            robot_folders: vec!["/robots".into(), "more".into()],
            sandbox: "none".into(),
            warm_hosts: false,
            ide_listen: false,
            ide_port: 9000,
            scan_arcs: true,
            colored_bullets: false,
            robot_names: false,
            robot_energy: true,
            explosions: false,
            ground: true,
            sentry_border: false,
            rates: true,
            sound: true,
            volume: 0.25,
            classic: true,
            rounds: 35,
            width: 1000,
            height: 1000,
            rules: Rules {
                gun_cooling_rate: 0.2,
                inactivity_time: 900,
                hide_enemy_names: true,
                sentry_border: 150,
            },
            watch: "server:7654".into(),
            watch_fingerprint: format!("sha256:{}", ["0F"; 32].join(":")),
            graphs_battle: true,
            frame_rate: 60,
        };
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(Settings::parse(&text).unwrap(), s);
    }

    #[test]
    fn finds_the_jars_in_the_folders() {
        let dir = std::env::temp_dir().join(format!("robocode-s-gui-jars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["b.jar", "a.JAR", "notes.txt", "sub/c.jar"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let s = Settings {
            robot_folders: vec![dir.clone(), dir.join("missing"), dir.join("sub/c.jar")],
            ..Settings::default()
        };
        let extra = [dir.join("b.jar"), PathBuf::from("/elsewhere/d.jar")];
        assert_eq!(
            s.robot_paths(&extra),
            vec![
                dir.join("a.JAR"),
                dir.join("b.jar"),
                dir.join("sub/c.jar"),
                PathBuf::from("/elsewhere/d.jar"),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_class_folders() {
        let dir =
            std::env::temp_dir().join(format!("robocode-s-gui-classes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["out/my/pkg", "empty/a", "hidden/.cache"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["out/r.jar", "out/my/pkg/Bot.class", "hidden/.cache/X.class"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let s = Settings {
            robot_folders: vec![dir.join("out"), dir.join("empty"), dir.join("hidden")],
            ..Settings::default()
        };
        assert_eq!(
            s.robot_paths(&[]),
            vec![dir.join("out/r.jar"), dir.join("out")]
        );
        assert!(Contents::of(&dir.join("out")).classes);
        assert_eq!(Contents::of(&dir.join("empty")), Contents::default());
        assert_eq!(Contents::of(&dir.join("hidden")), Contents::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
