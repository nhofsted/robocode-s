// SPDX-License-Identifier: EPL-2.0

//! Updates of an installed release (ADR 0022).
//!
//! The website describes the current release in `release.json`, made from
//! `web/src/lib/release.ts`: it changes when the download page does, which is after every
//! channel has the release. The programs fetch it at most once a day ([`State::due`]) and,
//! when it names a newer release, *ask* the user: nothing is downloaded before they agree.
//! What an update then does depends on how this copy was installed ([`Install::detect`]):
//!
//! - an archive (`.zip`, `.tar.gz`) or a macOS `.app` the user put somewhere: the new
//!   release's archive is downloaded, checked against the release's `SHA256SUMS` and put in
//!   place of the old files ([`replace`]); robots the user added to `robots` stay;
//! - the Windows installer's installation (also what winget installs): the new installer
//!   runs with `/UPDATE` once the program has exited, keeping the components chosen before;
//! - apt, dnf or Homebrew: they own the files, so the user is told the command to run;
//! - a .deb or .rpm installed without our repository: the download page.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// This build's release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where the current release is described; `$ROBOCODES_UPDATE_URL` names another file.
pub const RELEASE_URL: &str = "https://rumble.planethofstede.be/release.json";
pub const URL_VAR: &str = "ROBOCODES_UPDATE_URL";
/// Set (not empty): the programs don't check by themselves (`robocode-s update` still does).
pub const OFF_VAR: &str = "ROBOCODES_NO_UPDATE_CHECK";
/// The download page, for installations that can't be updated here.
pub const DOWNLOAD_PAGE: &str = "https://rumble.planethofstede.be/download";
/// How often the programs check by themselves.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Next to the installation's files during an update: the new release unpacked, and the
/// old files moved out of its way.
const NEW: &str = ".robocode-s-update-new";
const OLD: &str = ".robocode-s-update-old";

/// `release.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    /// The release's page (its notes).
    pub notes: String,
    /// The folder of its files: `<download>/<file>`, with `SHA256SUMS` among them.
    pub download: String,
    /// Its files' names.
    pub files: Vec<String>,
}

impl Release {
    /// Whether it is newer than this build.
    pub fn is_newer(&self) -> bool {
        newer(&self.version, VERSION)
    }

    /// The name of its file `robocode-s-<version>-<suffix>`, if it has one.
    fn file(&self, suffix: &str) -> Option<String> {
        let name = format!("robocode-s-{}-{suffix}", self.version);
        self.files.contains(&name).then_some(name)
    }
}

/// Whether release `a` (`0.2.10`) is newer than `b` (`0.2.9`): the numeric parts compared in
/// order; anything after a `-` is ignored.
pub fn newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect()
    }
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// Fetches `release.json`.
pub fn fetch() -> io::Result<Release> {
    let url = std::env::var(URL_VAR)
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| RELEASE_URL.to_string());
    let bytes = get(&url, 64 * 1024, Duration::from_secs(20))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{url}: {e}")))
}

fn get(url: &str, limit: u64, timeout: Duration) -> io::Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(timeout))
        .user_agent(format!(
            "robocode-s/{VERSION} ({}-{})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| io::Error::other(format!("{url}: {e}")))?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| io::Error::other(format!("{url}: {e}")))
}

/// What the programs remember between checks: `update.json` in the user data folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// When the last check was (Unix seconds), whether or not it got an answer.
    pub checked: u64,
    /// What it found.
    pub latest: Option<Release>,
    /// The release the user chose to skip.
    pub skipped: Option<String>,
    /// When the command line last said that there is a newer release.
    pub told: u64,
}

impl State {
    fn path() -> Option<PathBuf> {
        robocode_s_runner::local::user_data_dir().map(|d| d.join("update.json"))
    }

    /// The saved state; the default if there is none or it can't be read.
    pub fn load() -> State {
        Self::path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no user data folder"))?;
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(&path, &text)
    }

    /// Whether the daily check is due at `now`.
    pub fn due(&self, now: u64) -> bool {
        now < self.checked || now - self.checked >= CHECK_EVERY.as_secs()
    }

    /// Whether the user was told about the release to offer less than a day before `now`
    /// (or put it off): the programs don't tell again yet.
    pub fn told_recently(&self, now: u64) -> bool {
        now >= self.told && now - self.told < CHECK_EVERY.as_secs()
    }

    /// The release to offer: the latest found, if it is newer than this build and not
    /// skipped.
    pub fn offer(&self) -> Option<&Release> {
        self.latest
            .as_ref()
            .filter(|r| r.is_newer() && self.skipped.as_deref() != Some(r.version.as_str()))
    }
}

/// Unix seconds now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether the programs check by themselves: an installed release, and not turned off
/// with [`OFF_VAR`].
pub fn checks_by_itself() -> bool {
    std::env::var_os(OFF_VAR).is_none_or(|v| v.is_empty()) && Install::detect() != Install::Source
}

/// The daily check (blocking: run it off the UI thread): fetches `release.json` if a day
/// has passed since the last check, and returns the release to offer.
pub fn check_daily() -> Option<Release> {
    let mut state = State::load();
    let now = now();
    if state.due(now) {
        // Also when there is no answer, so that an offline machine doesn't try at every start.
        state.checked = now;
        if let Ok(release) = fetch() {
            state.latest = Some(release);
        }
        let _ = state.save();
    }
    state.offer().cloned()
}

/// How this copy was installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// Not an installed release (a build from source): nothing to update.
    Source,
    /// An archive unpacked in `root`.
    Archive { root: PathBuf },
    /// A macOS application bundle, `….app`.
    Bundle { app: PathBuf },
    /// The Windows installer's installation in `root`.
    Setup { root: PathBuf },
    /// A package manager's.
    Managed(Manager),
    /// A .deb or .rpm installed without our repository.
    Package,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Apt,
    Dnf,
    Homebrew,
}

impl Manager {
    /// The command that updates robocode-s.
    pub fn command(self) -> &'static str {
        match self {
            Manager::Apt => "sudo apt update && sudo apt install --only-upgrade robocode-s",
            Manager::Dnf => "sudo dnf upgrade --refresh robocode-s",
            Manager::Homebrew => "brew upgrade --cask robocode-s",
        }
    }
}

/// What updating takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum How {
    /// Download the release's `file` and install it here ([`download`], [`install`]).
    Here { file: String },
    /// The user runs this command.
    Command(&'static str),
    /// The user downloads it from [`DOWNLOAD_PAGE`].
    Page,
}

impl Install {
    /// This copy's installation.
    pub fn detect() -> Install {
        match robocode_s_runner::local::install_dir() {
            Some(root) => detect_at(&root, std::env::consts::OS, Path::new("/")),
            None => Install::Source,
        }
    }

    /// What updating it to `release` takes.
    pub fn how(&self, release: &Release) -> How {
        let arch = std::env::consts::ARCH;
        let suffix = match self {
            Install::Archive { .. } => match std::env::consts::OS {
                "windows" => format!("windows-{arch}.zip"),
                "macos" => format!("macos-{arch}.tar.gz"),
                _ => format!("linux-{arch}.tar.gz"),
            },
            Install::Bundle { .. } => format!("macos-{arch}.app.zip"),
            Install::Setup { .. } => format!("windows-{arch}-setup.exe"),
            Install::Managed(m) => return How::Command(m.command()),
            Install::Package | Install::Source => return How::Page,
        };
        release
            .file(&suffix)
            .map_or(How::Page, |file| How::Here { file })
    }
}

/// [`Install::detect`] for the installation in `root` on `os`, with the system's files under
/// `sys` (`/`; a test folder in tests).
fn detect_at(root: &Path, os: &str, sys: &Path) -> Install {
    match os {
        "windows" if root.join("uninstall.exe").is_file() => Install::Setup {
            root: root.to_path_buf(),
        },
        "macos" => match bundle_of(root) {
            Some(_)
                if [
                    "opt/homebrew/Caskroom/robocode-s",
                    "usr/local/Caskroom/robocode-s",
                ]
                .iter()
                .any(|p| sys.join(p).is_dir()) =>
            {
                Install::Managed(Manager::Homebrew)
            }
            Some(app) => Install::Bundle { app },
            None => Install::Archive {
                root: root.to_path_buf(),
            },
        },
        "linux" if root == sys.join("usr/lib/robocode-s") => {
            if sys
                .join("etc/apt/sources.list.d/robocode-s.sources")
                .is_file()
            {
                Install::Managed(Manager::Apt)
            } else if sys.join("etc/yum.repos.d/robocode-s.repo").is_file() {
                Install::Managed(Manager::Dnf)
            } else {
                Install::Package
            }
        }
        _ => Install::Archive {
            root: root.to_path_buf(),
        },
    }
}

/// The `….app` of an installation root `….app/Contents/Resources`.
fn bundle_of(root: &Path) -> Option<PathBuf> {
    let contents = root.parent().filter(|_| root.ends_with("Resources"))?;
    let app = contents
        .parent()
        .filter(|_| contents.ends_with("Contents"))?;
    app.extension()
        .is_some_and(|e| e == "app")
        .then(|| app.to_path_buf())
}

/// The folder downloads go to: `updates` in the user data folder.
fn updates_dir() -> io::Result<PathBuf> {
    robocode_s_runner::local::user_data_dir()
        .map(|d| d.join("updates"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no user data folder"))
}

/// Downloads `file` of `release`, checked against the release's `SHA256SUMS`, into the
/// user data folder; returns its path.
pub fn download(release: &Release, file: &str) -> io::Result<PathBuf> {
    download_to(release, file, &updates_dir()?)
}

fn download_to(release: &Release, file: &str, dir: &Path) -> io::Result<PathBuf> {
    let sums = get(
        &format!("{}/SHA256SUMS", release.download),
        1024 * 1024,
        Duration::from_secs(60),
    )?;
    let expected = sha256_of(&String::from_utf8_lossy(&sums), file).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{file} is not in the release's SHA256SUMS"),
        )
    })?;
    let bytes = get(
        &format!("{}/{file}", release.download),
        512 * 1024 * 1024,
        Duration::from_secs(30 * 60),
    )?;
    robocode_s_runner::jre::verify(&bytes, &expected)
        .map_err(|e| io::Error::new(e.kind(), format!("{file}: {e}")))?;
    fs::create_dir_all(dir)?;
    let path = dir.join(file);
    write_atomic(&path, &bytes)?;
    Ok(path)
}

/// The checksum of `file` in a `SHA256SUMS` (`<hex>  <name>` or `<hex> *<name>` lines).
fn sha256_of(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim_end().split_once(char::is_whitespace)?;
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// What [`install`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    /// The new release is in place; the programs started from now on are the new ones.
    Replaced,
    /// The installer starts as soon as this program exits: exit now.
    AfterExit,
}

/// Installs the downloaded `file` ([`download`]). For the Windows installer, `relaunch`
/// starts the GUI again when it is done.
pub fn install(install: &Install, file: &Path, relaunch: bool) -> io::Result<Done> {
    match install {
        Install::Archive { root } => {
            replace(root, file, Path::new("robots")).map(|()| Done::Replaced)
        }
        Install::Bundle { app } => {
            replace(app, file, Path::new("Contents/Resources/robots")).map(|()| Done::Replaced)
        }
        Install::Setup { root } => run_setup(root, file, relaunch).map(|()| Done::AfterExit),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this installation is updated by its package manager or by hand",
        )),
    }
}

/// Starts the Windows installer `setup` for the installation in `root`, detached: it waits
/// for the programs to exit (`/UPDATE`, `tools/release/installer.nsi`).
#[cfg(windows)]
fn run_setup(root: &Path, setup: &Path, relaunch: bool) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let mut command = Command::new(setup);
    command.args(["/S", "/UPDATE"]);
    if relaunch {
        command.arg("/RELAUNCH");
    }
    // NSIS takes /D= last and unquoted: the rest of the command line.
    command
        .raw_arg(format!("/D={}", root.display()))
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

#[cfg(not(windows))]
fn run_setup(_root: &Path, _setup: &Path, _relaunch: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the Windows installer runs on Windows",
    ))
}

/// Puts the release in `archive` (a `.tar.gz` or `.zip` with one top folder, as the release
/// has them) in place of the installation in `root`. The entries of `root` that the new
/// release has are replaced whole (so no file of the old release is left in `lib`); other
/// entries stay, and so do the robots in `robots` (relative to `root`) other than the
/// sample jars. On an error nothing is changed.
///
/// The old files are moved aside and then deleted: a running program can be moved, also on
/// Windows, where it can't be deleted until it exits ([`cleanup`] does that later). Robot
/// JVMs started from the installation must have stopped.
pub fn replace(root: &Path, archive: &Path, robots: &Path) -> io::Result<()> {
    let staging = root.join(NEW);
    let old = root.join(OLD);
    for leftover in [&staging, &old] {
        remove_all(leftover).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "couldn't remove {} from an earlier update: {e}",
                    leftover.display()
                ),
            )
        })?;
    }
    let result = unpack_and_swap(root, archive, robots, &staging, &old);
    let _ = remove_all(&staging);
    let _ = remove_all(&old);
    result
}

fn unpack_and_swap(
    root: &Path,
    archive: &Path,
    robots: &Path,
    staging: &Path,
    old: &Path,
) -> io::Result<()> {
    fs::create_dir_all(staging)?;
    unpack(archive, staging)?;
    let top = single_dir(staging)?;
    let resources = top.join(robots);
    let resources = resources.parent().unwrap_or(&top);
    if robocode_s_runner::local::install_root(resources).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a robocode-s release", archive.display()),
        ));
    }
    carry_robots(&root.join(robots), &top.join(robots))?;

    let mut names: Vec<OsString> = fs::read_dir(&top)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<_>>()?;
    names.sort();
    fs::create_dir(old)?;
    let (mut moved, mut placed) = (Vec::new(), Vec::new());
    let result: io::Result<()> = (|| {
        for name in &names {
            if root.join(name).symlink_metadata().is_ok() {
                fs::rename(root.join(name), old.join(name))?;
                moved.push(name);
            }
        }
        for name in &names {
            fs::rename(top.join(name), root.join(name))?;
            placed.push(name);
        }
        Ok(())
    })();
    if let Err(e) = result {
        for name in placed.iter().rev() {
            let _ = fs::rename(root.join(name), top.join(name));
        }
        for name in moved.iter().rev() {
            let _ = fs::rename(old.join(name), root.join(name));
        }
        return Err(io::Error::new(
            e.kind(),
            format!(
                "couldn't replace the files in {} ({e}); nothing was changed",
                root.display()
            ),
        ));
    }
    Ok(())
}

fn unpack(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = fs::File::open(archive)?;
    if archive.extension().is_some_and(|e| e == "zip") {
        zip::ZipArchive::new(file)
            .and_then(|mut z| z.extract(dest))
            .map_err(io::Error::other)
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(file))).unpack(dest)
    }
}

/// The one folder in `dir`.
fn single_dir(dir: &Path) -> io::Result<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<_>>()?;
    match entries.as_slice() {
        [top] if top.is_dir() => Ok(top.clone()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the release archive doesn't have one top folder",
        )),
    }
}

/// Copies the user's robots from the old `robots` folder into the new one: all but the
/// sample jars and what the new one has.
fn carry_robots(old: &Path, new: &Path) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(old) else {
        return Ok(());
    };
    fs::create_dir_all(new)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let lossy = name.to_string_lossy();
        let sample = lossy.starts_with("samples") && lossy.ends_with(".jar");
        if sample || new.join(&name).symlink_metadata().is_ok() {
            continue;
        }
        copy_all(&entry.path(), &new.join(&name))?;
    }
    Ok(())
}

fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    #[cfg(unix)]
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
    }
    if meta.is_dir() || (meta.file_type().is_symlink() && from.is_dir()) {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(drop)
    }
}

/// Removes what an update left: the old files in the installation that couldn't be deleted
/// while the program ran (Windows), and the downloads. Cheap; call it at start.
pub fn cleanup() {
    let roots = match Install::detect() {
        Install::Archive { root } => Some(root),
        Install::Bundle { app } => Some(app),
        _ => None,
    };
    if let Some(root) = roots {
        let _ = remove_all(&root.join(OLD));
        let _ = remove_all(&root.join(NEW));
    }
    // The downloads of this release or older.
    if let Ok(entries) = updates_dir().and_then(fs::read_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name
                .strip_prefix("robocode-s-")
                .and_then(|v| v.split('-').next())
                .unwrap_or("");
            if !newer(version, VERSION) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn remove_all(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
