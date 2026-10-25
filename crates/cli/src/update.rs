// SPDX-License-Identifier: EPL-2.0

//! `robocode-s update`, and the other commands' daily notice of a newer release (ADR 0022).

use robocode_s_update::{self as update, DOWNLOAD_PAGE, Done, How, Install, State, VERSION};
use std::io::{BufRead, IsTerminal, Write};

pub const USAGE: &str = "\
usage: robocode-s update [--check] [--yes]

Looks for a newer release of robocode-s and offers to install it. An installation from an
archive (.zip, .tar.gz, the macOS .app) or from the Windows installer is updated here;
for apt, dnf and Homebrew it names the command to run.

The other commands look once a day by themselves and say so on a terminal when there is
a newer release; ROBOCODES_NO_UPDATE_CHECK=1 turns that off.

  --check  only say whether there is a newer release
  --yes    install it without asking
";

pub fn run(args: &[String]) -> Result<(), String> {
    let (mut check, mut yes) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--yes" | "-y" => yes = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    let release = update::fetch().map_err(|e| format!("couldn't look for a newer release: {e}"))?;
    let mut state = State::load();
    state.checked = update::now();
    state.latest = Some(release.clone());
    let _ = state.save();
    if !release.is_newer() {
        println!("robocode-s {VERSION} is the current release.");
        return Ok(());
    }
    println!(
        "robocode-s {} is out (this is {VERSION}): {}",
        release.version, release.notes
    );
    if check {
        return Ok(());
    }
    let install = Install::detect();
    match install.how(&release) {
        How::Command(command) => println!("Update it with\n  {command}"),
        How::Page if install == Install::Source => {
            println!("This is a build from source: pull and build again to update it.");
        }
        How::Page => println!("Download it from {DOWNLOAD_PAGE}"),
        How::Here { file } => {
            if !yes && !confirm(&format!("Install robocode-s {}? [y/N] ", release.version))? {
                return Ok(());
            }
            eprintln!("Downloading {file}...");
            let path = update::download(&release, &file).map_err(|e| e.to_string())?;
            match update::install(&install, &path, false).map_err(|e| e.to_string())? {
                Done::Replaced => println!("robocode-s {} is installed.", release.version),
                Done::AfterExit => println!(
                    "The installer updates robocode-s to {} in the background now.",
                    release.version
                ),
            }
        }
    }
    Ok(())
}

/// Asks on the terminal; without one, refuses rather than guess.
fn confirm(question: &str) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err("not installing without asking: run it on a terminal, or pass --yes".into());
    }
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// For the other commands, on a terminal: at most once a day, says that there is a newer
/// release (as the last check found it) and looks again in the background.
pub fn daily_notice() {
    update::cleanup();
    if !std::io::stderr().is_terminal() || !update::checks_by_itself() {
        return;
    }
    let mut state = State::load();
    let now = update::now();
    if let Some(release) = state.offer()
        && !state.told_recently(now)
    {
        eprintln!(
            "robocode-s {} is out (this is {VERSION}): `robocode-s update` installs it.",
            release.version
        );
        state.told = now;
        let _ = state.save();
    }
    if state.due(now) {
        // Not waited for: a short command may end first, and the next one looks again.
        std::thread::spawn(update::check_daily);
    }
}
