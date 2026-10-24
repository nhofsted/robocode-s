// SPDX-License-Identifier: EPL-2.0

//! The upload token on disk (§7.2): `rumble.json` next to the GUI's `gui.json`.
//!
//! The config directory logic is replicated from `robocode-s-gui`'s `Settings::path` (that
//! crate will depend on this one, and the CLI shouldn't pull in the GUI): `$XDG_CONFIG_HOME`
//! (else `~/.config`) on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on
//! Windows, then `robocode-s/`. `ROBOCODES_RUMBLE_CONFIG` names another file;
//! `ROBOCODES_RUMBLE_TOKEN` overrides the token (for headless machines).

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONFIG_VAR: &str = "ROBOCODES_RUMBLE_CONFIG";
pub const TOKEN_VAR: &str = "ROBOCODES_RUMBLE_TOKEN";

/// The content of `rumble.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenFile {
    /// `https://<domain>`
    pub server: String,
    /// `rsu_...`
    pub token: String,
    pub login: String,
    /// ISO 8601 UTC, see [`now_iso`].
    pub saved_at: String,
}

/// Where a token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Env,
    File,
}

/// The token to use.
#[derive(Clone, Debug, PartialEq)]
pub struct Credentials {
    pub token: String,
    /// The server of `rumble.json`, if there is one (also with a token from the environment).
    pub server: Option<String>,
    pub login: Option<String>,
    pub source: Source,
}

/// `rumble.json`'s path, reading variables through `var` (empty values count as unset).
pub fn path_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|v| !v.is_empty());
    if let Some(file) = var(CONFIG_VAR) {
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
    Some(dir.join("robocode-s").join("rumble.json"))
}

/// `rumble.json`'s path from the process environment.
pub fn path() -> Option<PathBuf> {
    path_from(&|name| std::env::var_os(name))
}

/// Reads the file; `None` if it doesn't exist.
pub fn read(path: &Path) -> io::Result<Option<TokenFile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes the file atomically (a temp file in the same directory, then a rename), created
/// with mode 0600 on Unix.
pub fn write(path: &Path, file: &TokenFile) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map_or_else(
        || "rumble.json".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut out = options.open(&tmp)?;
        let mut text = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
        text.push('\n');
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Deletes the file (logout); a missing file is fine.
pub fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The token to use: `ROBOCODES_RUMBLE_TOKEN` if set, else the file's.
pub fn resolve_from(var: &dyn Fn(&str) -> Option<OsString>) -> io::Result<Option<Credentials>> {
    let file = match path_from(var) {
        Some(path) => read(&path)?,
        None => None,
    };
    if let Some(token) = var(TOKEN_VAR)
        .and_then(|t| t.into_string().ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        return Ok(Some(Credentials {
            token,
            server: file.as_ref().map(|f| f.server.clone()),
            login: file.map(|f| f.login),
            source: Source::Env,
        }));
    }
    Ok(file.map(|f| Credentials {
        token: f.token,
        server: Some(f.server),
        login: Some(f.login),
        source: Source::File,
    }))
}

/// [`resolve_from`] on the process environment.
pub fn resolve() -> io::Result<Option<Credentials>> {
    resolve_from(&|name| std::env::var_os(name))
}

/// The current time as `2026-10-24T13:05:09Z`.
pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    iso(secs)
}

/// Unix seconds as `2026-10-24T13:05:09Z`.
pub fn iso(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rumble-client-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> TokenFile {
        TokenFile {
            server: "https://rumble.example".into(),
            token: "rsu_abc".into(),
            login: "nick".into(),
            saved_at: "2026-10-24T13:05:09Z".into(),
        }
    }

    #[test]
    fn config_override_names_the_file() {
        let var = env(&[(CONFIG_VAR, "/x/y.json"), ("HOME", "/h")]);
        assert_eq!(path_from(&var), Some(PathBuf::from("/x/y.json")));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn default_path_sits_next_to_gui_json() {
        let var = env(&[("XDG_CONFIG_HOME", "/c"), ("HOME", "/h")]);
        assert_eq!(
            path_from(&var),
            Some(PathBuf::from("/c/robocode-s/rumble.json"))
        );
        let var = env(&[("HOME", "/h")]);
        assert_eq!(
            path_from(&var),
            Some(PathBuf::from("/h/.config/robocode-s/rumble.json"))
        );
        assert_eq!(path_from(&env(&[])), None);
    }

    #[test]
    fn write_read_remove() {
        let dir = temp("rw");
        let path = dir.join("sub/rumble.json");
        assert_eq!(read(&path).unwrap(), None);
        write(&path, &sample()).unwrap();
        assert_eq!(read(&path).unwrap(), Some(sample()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"savedAt\""));
        // Overwrite leaves no temp file.
        let mut other = sample();
        other.token = "rsu_def".into();
        write(&path, &other).unwrap();
        assert_eq!(read(&path).unwrap(), Some(other));
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        remove(&path).unwrap();
        remove(&path).unwrap();
        assert_eq!(read(&path).unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("mode");
        let path = dir.join("rumble.json");
        write(&path, &sample()).unwrap();
        write(&path, &sample()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn env_token_overrides_the_file() {
        let dir = temp("env");
        let path = dir.join("rumble.json");
        let p = path.to_str().unwrap();
        let none = resolve_from(&env(&[(CONFIG_VAR, p)])).unwrap();
        assert_eq!(none, None);
        write(&path, &sample()).unwrap();
        let from_file = resolve_from(&env(&[(CONFIG_VAR, p)])).unwrap().unwrap();
        assert_eq!(from_file.token, "rsu_abc");
        assert_eq!(from_file.source, Source::File);
        let from_env = resolve_from(&env(&[(CONFIG_VAR, p), (TOKEN_VAR, " rsu_env\n")]))
            .unwrap()
            .unwrap();
        assert_eq!(from_env.token, "rsu_env");
        assert_eq!(from_env.source, Source::Env);
        assert_eq!(from_env.server.as_deref(), Some("https://rumble.example"));
        // Env token alone, no file.
        let alone = resolve_from(&env(&[
            (CONFIG_VAR, dir.join("nope.json").to_str().unwrap()),
            (TOKEN_VAR, "rsu_env"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(alone.server, None);
        // A broken file is an error, not silence.
        std::fs::write(&path, "{").unwrap();
        assert!(resolve_from(&env(&[(CONFIG_VAR, p)])).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_782_306_309), "2026-06-24T13:05:09Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(now_iso().len(), 20);
    }
}
