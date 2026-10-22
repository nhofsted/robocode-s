// SPDX-License-Identifier: EPL-2.0

//! What a run leaves on disk, and what it cleans up (design §6.5, §6.7, §6.8): a robot's
//! persistent data files, the folders of battles and warm hosts, and the ones a crashed or
//! killed run left behind.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Set (to anything but `0` or nothing), battles and warm hosts keep their folders: the
/// robots' homes with classic's files, their output and error output, for diagnosis.
pub const KEEP_ENV: &str = "ROBOCODES_KEEP_BATTLES";

/// Whether [`KEEP_ENV`] asks for battle folders to stay.
pub fn keep_battles() -> bool {
    std::env::var_os(KEEP_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// A folder without a live process's id in its name this old was left behind by a crash.
pub const STALE_RUN: Duration = Duration::from_secs(24 * 60 * 60);

/// Removes from `dir` (the folder of battles, or of pools) what runs left behind: folders
/// named after a process (`<pid>` or `<stamp>-<pid>`) that no longer runs, and others that
/// were last changed more than [`STALE_RUN`] ago. Folders of running processes (another
/// GUI or CLI at the same time) stay.
pub fn remove_stale_runs(dir: &Path) {
    remove_stale_with(dir, STALE_RUN, process_running);
}

fn remove_stale_with(dir: &Path, age: Duration, running: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let stale = match pid_of(&entry.file_name().to_string_lossy()) {
            Some(pid) => !running(pid),
            None => metadata
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|elapsed| elapsed > age),
        };
        if stale {
            let _ = if metadata.is_dir() {
                std::fs::remove_dir_all(entry.path())
            } else {
                std::fs::remove_file(entry.path())
            };
        }
    }
}

/// The process id a folder is named after: the number after its last `-`, or the whole name.
fn pid_of(name: &str) -> Option<u32> {
    name.rsplit('-').next()?.parse().ok()
}

/// Whether the process `pid` runs (one we may not signal does).
#[cfg(unix)]
#[allow(unsafe_code)] // one system call, on a number
pub fn process_running(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    (unsafe { libc::kill(pid, 0) == 0 })
        || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 calls, on a handle this function closes
pub fn process_running(pid: u32) -> bool {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: a handle it gives is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        // SAFETY: reads the thread's last error.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let mut code = 0;
    // SAFETY: `process` is a process handle.
    let running = unsafe { GetExitCodeProcess(process, &mut code) } != 0 && code == STILL_ACTIVE;
    // SAFETY: as above.
    drop(unsafe { OwnedHandle::from_raw_handle(process) });
    running
}

/// Where classic keeps a robot's data files below its home: `<package>/<Class>.data`, in
/// this folder.
const DATA_IN_HOME: &str = "robots/.data";

/// The most a robot's data files may take when they are copied back, much more than the
/// 200 KB classic's quota lets a robot write: a robot with no sandbox's checks is held to it.
const MAX_DATA_BYTES: u64 = 4 * 1024 * 1024;

/// The persistent data folder of the robot `lookup` (`jk.mega.DrussGT 3.1.16`, with its
/// version as classic names a robot) below `root`.
pub fn robot_data_dir(root: &Path, lookup: &str) -> PathBuf {
    let name: String = lookup
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    root.join(name)
}

/// Puts the robot's persistent data files `data` (if it has any yet) where classic looks
/// for them in `home`, before the robot starts. Returns whether there were any.
pub fn copy_data_in(data: &Path, home: &Path) -> io::Result<bool> {
    if !data.is_dir() {
        return Ok(false);
    }
    copy_tree(data, &home.join(DATA_IN_HOME), &mut 0)?;
    Ok(true)
}

/// Keeps the data files the robot left in `home` as its persistent ones, replacing the
/// old: a robot with the same data folder in the same battle, or one finishing meanwhile,
/// is overwritten by whichever ends last. `had` says that [`copy_data_in`] gave the robot
/// files: if it removed them all, they go.
pub fn copy_data_out(home: &Path, data: &Path, had: bool) -> io::Result<()> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let from = home.join(DATA_IN_HOME);
    if !from.is_dir() {
        return if had {
            std::fs::remove_dir_all(data)
        } else {
            Ok(())
        };
    }
    let name = data.file_name().unwrap_or_default().to_string_lossy();
    let tmp = data.with_file_name(format!(
        ".tmp-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let copied = copy_tree(&from, &tmp, &mut 0).and_then(|()| {
        let _ = std::fs::remove_dir_all(data);
        std::fs::rename(&tmp, data)
    });
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    copied
}

/// Copies the files and folders of `from` into `to`; what is neither (a link a robot made
/// to read what the runner may read and it may not) is left out. Fails when more than
/// [`MAX_DATA_BYTES`] come.
fn copy_tree(from: &Path, to: &Path, copied: &mut u64) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, copied)?;
        } else if kind.is_file() {
            *copied += entry.metadata()?.len();
            if *copied > MAX_DATA_BYTES {
                return Err(io::Error::other("more data files than a robot may keep"));
            }
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Removes a robot's home (or a battle's folder): not while [`KEEP_ENV`] asks to keep
/// them. A process that just ended may still hold a file for a moment on Windows.
pub fn remove_home(home: &Path) {
    if keep_battles() {
        return;
    }
    for _ in 0..10 {
        if std::fs::remove_dir_all(home).is_ok() || !home.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("robocodes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_old(path: &Path) {
        let old = std::time::SystemTime::now() - 2 * STALE_RUN;
        std::fs::File::open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn names_carry_the_process_id() {
        assert_eq!(pid_of("1792688873095-1347399"), Some(1347399));
        assert_eq!(pid_of("1347399"), Some(1347399));
        assert_eq!(pid_of("melee-samples.jsonl.gz"), None);
        assert_eq!(pid_of("describe"), None);
    }

    #[test]
    fn stale_runs_are_removed_and_running_ones_kept() {
        let dir = temp("stale-runs");
        let make = |name: &str| std::fs::create_dir_all(dir.join(name).join("robot-0")).unwrap();
        for name in ["100-1", "100-2", "3", "kept-by-age", "removed-by-age"] {
            make(name);
        }
        set_old(&dir.join("removed-by-age"));
        // Even a day old, a live process's folder stays.
        set_old(&dir.join("100-1"));
        remove_stale_with(&dir, STALE_RUN, |pid| pid == 1 || pid == 3);
        let left = |name: &str| dir.join(name).exists();
        assert!(left("100-1"), "live process");
        assert!(!left("100-2"), "dead process");
        assert!(left("3"), "live process, pool style");
        assert!(left("kept-by-age"), "recent, no process");
        assert!(!left("removed-by-age"), "old, no process");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn this_process_runs_and_a_made_up_one_does_not() {
        assert!(process_running(std::process::id()));
        assert!(!process_running(u32::MAX - 1));
    }

    #[test]
    fn data_folders_are_named_by_robot_and_version() {
        let root = Path::new("/d");
        assert_eq!(
            robot_data_dir(root, "jk.mega.DrussGT 3.1.16"),
            root.join("jk.mega.DrussGT 3.1.16")
        );
        assert_eq!(robot_data_dir(root, "a.B*"), root.join("a.B_"));
        assert_eq!(robot_data_dir(root, "../x"), root.join(".._x"));
    }

    #[test]
    fn data_files_go_out_with_the_robot_and_come_back() {
        let dir = temp("data");
        let (home, data) = (dir.join("home"), dir.join("data").join("sample.Duck 1.0"));
        let in_home = home.join(DATA_IN_HOME).join("sample/Duck.data");
        std::fs::create_dir_all(&in_home).unwrap();
        std::fs::write(in_home.join("count.dat"), "1").unwrap();
        std::fs::create_dir_all(data.parent().unwrap()).unwrap();
        // The first battle: nothing to give, the robot's files are kept.
        assert!(!copy_data_in(&data, &home).unwrap());
        copy_data_out(&home, &data, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(data.join("sample/Duck.data/count.dat")).unwrap(),
            "1"
        );
        // The next one: they are in the new home, and its changes replace them.
        let next = dir.join("next");
        assert!(copy_data_in(&data, &next).unwrap());
        let file = next.join(DATA_IN_HOME).join("sample/Duck.data/count.dat");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "1");
        std::fs::write(&file, "2").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", file.with_file_name("link")).unwrap();
        copy_data_out(&next, &data, true).unwrap();
        let kept = data.join("sample/Duck.data");
        assert_eq!(
            std::fs::read_to_string(kept.join("count.dat")).unwrap(),
            "2"
        );
        assert!(!kept.join("link").exists(), "links are not followed");
        // A robot that removed its files has none left.
        std::fs::remove_dir_all(next.join(DATA_IN_HOME)).unwrap();
        copy_data_out(&next, &data, true).unwrap();
        assert!(!data.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn too_many_data_files_are_not_kept() {
        let dir = temp("data-big");
        let (home, data) = (dir.join("home"), dir.join("data").join("r"));
        let folder = home.join(DATA_IN_HOME);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("big"), vec![0u8; MAX_DATA_BYTES as usize + 1]).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("old"), "x").unwrap();
        assert!(copy_data_out(&home, &data, true).is_err());
        assert!(data.join("old").exists(), "what there was stays");
        assert_eq!(std::fs::read_dir(dir.join("data")).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
