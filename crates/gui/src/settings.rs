// SPDX-License-Identifier: EPL-2.0

//! The options (the gear's page), saved as JSON in the platform's config directory:
//! `$XDG_CONFIG_HOME/robocode-s/gui.json` (else `~/.config/…`) on Linux,
//! `~/Library/Application Support/robocode-s/gui.json` on macOS and
//! `%APPDATA%\robocode-s\gui.json` on Windows; `ROBOCODES_GUI_CONFIG` overrides it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The options. Missing fields take their defaults, so older files still load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folders scanned for robot jars.
    pub robot_folders: Vec<PathBuf>,
    /// Draw each robot's radar scan arc.
    pub scan_arcs: bool,
    /// Bullets in the robots' bullet colours rather than all in one.
    pub colored_bullets: bool,
    /// The defaults for new battles.
    pub classic: bool,
    pub rounds: i32,
    pub width: i32,
    pub height: i32,
    pub rules: Rules,
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
            // The samples of a built source tree, as `Environment::from_env` assumes.
            robot_folders: vec!["robots/samples/build/libs".into()],
            scan_arcs: false,
            colored_bullets: true,
            classic: false,
            rounds: 10,
            width: 800,
            height: 600,
            rules: Rules::default(),
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
        serde_json::from_str(text)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("no config directory (HOME is not set)")?;
        let fail = |e: std::io::Error| format!("{}: {e}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text + "\n").map_err(fail)
    }

    /// The robot jars: the `.jar` files in the robot folders (a folder entry may also name
    /// a jar), then `extra` (from `ROBOCODES_ROBOT_JARS`); each once, folders sorted.
    pub fn robot_jars(&self, extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut jars = Vec::new();
        for folder in &self.robot_folders {
            if is_jar(folder) && folder.is_file() {
                jars.push(folder.clone());
                continue;
            }
            let Ok(entries) = std::fs::read_dir(folder) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .filter_map(|e| Some(e.ok()?.path()))
                .filter(|p| is_jar(p) && p.is_file())
                .collect();
            found.sort();
            jars.extend(found);
        }
        jars.extend(extra.iter().cloned());
        let mut seen = std::collections::HashSet::new();
        jars.retain(|j| seen.insert(j.clone()));
        jars
    }
}

fn is_jar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jar"))
}

/// `ROBOCODES_ROBOT_JARS`: extra robot jars (a path list), as for `robocode-s battle`.
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
    }

    #[test]
    fn round_trips() {
        let s = Settings {
            robot_folders: vec!["/robots".into(), "more".into()],
            scan_arcs: true,
            colored_bullets: false,
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
            s.robot_jars(&extra),
            vec![
                dir.join("a.JAR"),
                dir.join("b.jar"),
                dir.join("sub/c.jar"),
                PathBuf::from("/elsewhere/d.jar"),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
