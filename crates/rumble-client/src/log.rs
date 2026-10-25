// SPDX-License-Identifier: EPL-2.0

//! The log file: what an unattended run leaves behind when it ends overnight. The GUI and
//! `robocode-s rumble run` append to one file, `<user data>/logs/robocode-s.log`
//! (`%LOCALAPPDATA%\robocode-s\logs` on Windows; another file with `$ROBOCODES_LOG`):
//! their start and exit, and panics with a backtrace (the rumble's session lines are in
//! `<data>/rumble/log.txt`). A process that started and left no exit line and no panic was
//! killed, aborted (out of memory, a stack overflow: those go to stderr, which the Windows
//! GUI points here), or the machine stopped; the next start says so.
//!
//! Lines are `<UTC time> <program>[<pid>] <text>`, continuation lines indented. The file is
//! rotated at start once it passes [`MAX_BYTES`], to `robocode-s.1.log`. Nothing is written
//! before [`init`], so tests and other commands leave no file.

use crate::tokenfile::now_iso;
use robocode_s_runner::local::user_data_dir;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Another log file than the default.
pub const LOG_VAR: &str = "ROBOCODES_LOG";

/// The size past which the next start rotates the file.
pub const MAX_BYTES: u64 = 10 << 20;

/// How much of the file's end [`init`] reads for the previous session.
const TAIL_BYTES: u64 = 256 << 10;

struct Log {
    program: &'static str,
    file: Mutex<File>,
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Whether stderr goes into the log (the Windows GUI without a terminal), so the panic
/// hook doesn't write a panic twice.
static STDERR_IS_LOG: AtomicBool = AtomicBool::new(false);

/// The log file: `$ROBOCODES_LOG`, else `logs/robocode-s.log` in the user data folder.
pub fn path() -> Option<PathBuf> {
    match std::env::var_os(LOG_VAR).filter(|v| !v.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None => user_data_dir().map(|d| d.join("logs").join("robocode-s.log")),
    }
}

/// Opens the log for `program` (rotating it when big), notes the start and how the
/// previous session of `program` ended if it left no word, and installs the panic hook.
/// Returns the file, or `None` when it can't be opened (the program runs without it).
pub fn init(program: &'static str) -> Option<PathBuf> {
    let path = path()?;
    if LOG.get().is_some() {
        return Some(path);
    }
    let previous = open(&path, program).ok()?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    if LOG
        .set(Log {
            program,
            file: Mutex::new(file),
        })
        .is_err()
    {
        return Some(path);
    }
    if let Some(note) = previous {
        line(&note);
    }
    line(&format!(
        "started: version {}, engine {}, on {} {}",
        env!("CARGO_PKG_VERSION"),
        robocode_s_engine::ENGINE_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let at = info.location().map_or_else(String::new, |l| {
            format!(" at {}:{}:{}", l.file(), l.line(), l.column())
        });
        line(&format!(
            "panicked on thread '{}'{at}: {message}\nbacktrace:\n{}",
            thread.name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture()
        ));
        if !STDERR_IS_LOG.load(Ordering::Relaxed) {
            default(info);
        }
    }));
    Some(path)
}

/// Creates the folder, rotates a big file, and reads how the last session of `program`
/// ended: the note for one that left no exit line.
fn open(path: &Path, program: &str) -> std::io::Result<Option<String>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tail = String::new();
    if let Ok(mut f) = File::open(path) {
        let len = f.metadata()?.len();
        if len > MAX_BYTES {
            drop(f);
            let rotated = path.with_extension("1.log");
            let _ = std::fs::remove_file(&rotated);
            std::fs::rename(path, &rotated)?;
            f = File::open(&rotated)?;
        }
        let len = f.metadata()?.len();
        f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        tail = String::from_utf8_lossy(&bytes).into_owned();
    }
    Ok(unfinished(&tail, program))
}

/// The note for the last session of `program` in `tail` when it neither exited nor
/// panicked.
fn unfinished(tail: &str, program: &str) -> Option<String> {
    let me = format!(" {program}[");
    let mut last: Option<(&str, &str)> = None; // (time, pid) of a session still open
    for l in tail.lines() {
        let Some((time, rest)) = l.split_once(&me) else {
            continue;
        };
        let Some((pid, text)) = rest.split_once("] ") else {
            continue;
        };
        if text.starts_with("started:") {
            last = Some((time, pid));
        } else if last.is_some_and(|(_, p)| p == pid)
            && (text.starts_with("exited") || text.starts_with("panicked"))
        {
            last = None;
        }
    }
    last.map(|(time, pid)| {
        format!(
            "the previous {program} (pid {pid}, started {time}) left no exit line: it was \
             killed, aborted (out of memory?) or the machine stopped; rumble/log.txt shows \
             what it did last"
        )
    })
}

/// Appends `text` (each further line indented), if the log is open.
pub fn line(text: &str) {
    let Some(log) = LOG.get() else {
        return;
    };
    let mut out = format!(
        "{} {}[{}] {}\n",
        now_iso(),
        log.program,
        std::process::id(),
        text.trim_end().replace('\n', "\n    ")
    );
    if out.contains('\r') {
        out = out.replace('\r', "");
    }
    // A panic while the lock was held leaves it poisoned; the file is still fine.
    let mut file = log
        .file
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = file.write_all(out.as_bytes());
}

/// Notes a normal exit with `code`.
pub fn exit(code: &str) {
    line(&format!("exited: {code}"));
}

/// Another handle on the log file for stderr (appending), marking stderr as going there.
pub fn stderr_file() -> Option<File> {
    let file = LOG.get()?.file.lock().ok()?.try_clone().ok()?;
    STDERR_IS_LOG.store(true, Ordering::Relaxed);
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::unfinished;

    const GUI: &str = "robocode-s-gui";

    #[test]
    fn a_clean_exit_or_a_panic_ends_a_session() {
        assert_eq!(unfinished("", GUI), None);
        let clean = "2026-10-25T01:00:00Z robocode-s-gui[7] started: version 0.2.1\n\
                     2026-10-25T02:00:00Z robocode-s-gui[7] exited: 0\n";
        assert_eq!(unfinished(clean, GUI), None);
        let panic = "2026-10-25T01:00:00Z robocode-s-gui[7] started: version 0.2.1\n\
                     2026-10-25T02:00:00Z robocode-s-gui[7] panicked on thread 'main': x\n    \
                     backtrace:\n";
        assert_eq!(unfinished(panic, GUI), None);
    }

    #[test]
    fn a_session_without_a_word_is_noted() {
        let tail = "2026-10-25T01:00:00Z robocode-s-gui[7] started: version 0.2.1\n\
                    2026-10-25T01:30:00Z robocode-s[9] started: version 0.2.1\n\
                    2026-10-25T01:31:00Z robocode-s[9] exited: 0\n\
                    stray stderr text\n";
        let note = unfinished(tail, GUI).unwrap();
        assert!(
            note.contains("pid 7, started 2026-10-25T01:00:00Z"),
            "{note}"
        );
        assert_eq!(unfinished(tail, "robocode-s"), None);
    }

    #[test]
    fn only_the_last_session_counts() {
        let tail = "2026-10-24T01:00:00Z robocode-s-gui[5] started: version 0.2.1\n\
                    2026-10-25T01:00:00Z robocode-s-gui[7] started: version 0.2.1\n\
                    2026-10-25T02:00:00Z robocode-s-gui[7] exited: 0\n";
        assert_eq!(unfinished(tail, GUI), None);
    }
}
