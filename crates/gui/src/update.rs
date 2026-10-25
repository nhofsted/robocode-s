// SPDX-License-Identifier: EPL-2.0

//! A newer release: the daily check, the notice that offers it, and installing it when the
//! user agrees (crates/update, ADR 0022).

use crate::rumble::open_url;
use crate::shell;
use crate::window::BAR_HEIGHT;
use eframe::egui::{self, RichText};
use robocode_s_update::{self as update, Done, How, Install, Release, State, VERSION};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub struct Updater {
    install: Install,
    phase: Phase,
}

enum Phase {
    Idle,
    /// Looking for a newer release; `asked`: the user asked (else the daily check).
    Looking {
        rx: Receiver<Result<Option<Release>, String>>,
        asked: bool,
    },
    /// The notice offers it.
    Offer(Release),
    /// Downloading and installing it. `exe` is this program's path, taken before its file is
    /// replaced (on Linux the running program's path then names the moved old file).
    Installing {
        version: String,
        rx: Receiver<Result<Done, String>>,
        exe: Option<PathBuf>,
    },
    /// What the notice says until it is dismissed: up to date, or what went wrong.
    Note(String),
}

/// What the app must do after [`Updater::ui`].
#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    /// Stop the robot JVMs started from the installation, then call [`Updater::install`].
    Install,
    /// Close the window: the Windows installer takes over, and starts the app again.
    Exit,
    /// Start this program again (the new release) and close the window.
    Restart(PathBuf),
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub fn new() -> Self {
        Self {
            install: Install::detect(),
            phase: Phase::Idle,
        }
    }

    /// How this copy was installed.
    pub fn installation(&self) -> &Install {
        &self.install
    }

    /// The daily check, in the background (the option "Look for a newer release once a
    /// day"). Offers a release only once a day, whatever the user answered.
    pub fn start_daily(&mut self, ctx: &egui::Context) {
        if !update::checks_by_itself() {
            return;
        }
        self.look(ctx, false, || {
            let release = update::check_daily();
            Ok(release.filter(|_| !State::load().told_recently(update::now())))
        });
    }

    /// Looks now, because the user asked (also for a skipped release).
    pub fn look_now(&mut self, ctx: &egui::Context) {
        self.look(ctx, true, || {
            let release = update::fetch().map_err(|e| e.to_string())?;
            let mut state = State::load();
            state.checked = update::now();
            state.latest = Some(release.clone());
            let _ = state.save();
            Ok(release.is_newer().then_some(release))
        });
    }

    fn look(
        &mut self,
        ctx: &egui::Context,
        asked: bool,
        look: impl FnOnce() -> Result<Option<Release>, String> + Send + 'static,
    ) {
        if matches!(self.phase, Phase::Looking { .. } | Phase::Installing { .. }) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(look());
            ctx.request_repaint();
        });
        self.phase = Phase::Looking { rx, asked };
    }

    /// Whether a look the user asked for, or an update, is under way.
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Looking { asked: true, .. } | Phase::Installing { .. }
        )
    }

    /// Says `text` in the notice.
    pub fn note(&mut self, text: String) {
        self.phase = Phase::Note(text);
    }

    /// Downloads and installs the offered release, in the background.
    pub fn install(&mut self, ctx: &egui::Context) {
        let Phase::Offer(release) = &self.phase else {
            return;
        };
        let How::Here { file } = self.install.how(release) else {
            return;
        };
        let (release, install) = (release.clone(), self.install.clone());
        let version = release.version.clone();
        let exe = std::env::current_exe().ok();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = update::download(&release, &file)
                .and_then(|path| update::install(&install, &path, true))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.phase = Phase::Installing { version, rx, exe };
    }

    fn poll(&mut self) -> Action {
        match &self.phase {
            Phase::Looking { rx, asked } => {
                let asked = *asked;
                match rx.try_recv() {
                    Err(TryRecvError::Empty) => {}
                    Ok(Ok(Some(release))) => self.phase = Phase::Offer(release),
                    Ok(Ok(None)) if asked => {
                        self.phase =
                            Phase::Note(format!("robocode-s {VERSION} is the current release."));
                    }
                    Ok(Err(e)) if asked => {
                        self.phase = Phase::Note(format!("Couldn't look for a newer release: {e}"));
                    }
                    _ => self.phase = Phase::Idle,
                }
            }
            Phase::Installing { version, rx, exe } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(Ok(Done::AfterExit)) => return Action::Exit,
                Ok(Ok(Done::Replaced)) => match exe.clone() {
                    Some(exe) => return Action::Restart(exe),
                    None => {
                        self.phase = Phase::Note(format!(
                            "robocode-s {version} is installed: start it again to use it."
                        ));
                    }
                },
                Ok(Err(e)) => self.phase = Phase::Note(format!("Couldn't update robocode-s: {e}")),
                Err(TryRecvError::Disconnected) => {
                    self.phase = Phase::Note("Couldn't update robocode-s.".into());
                }
            },
            _ => {}
        }
        Action::None
    }

    /// The notice at the top right, while there is something to say. `wait`: why the update
    /// can't be installed now (a battle runs, the rumble is being run).
    pub fn ui(&mut self, ctx: &egui::Context, wait: Option<&str>) -> Action {
        let action = self.poll();
        if action != Action::None {
            return action;
        }
        let mut action = Action::None;
        let (mut close, mut later, mut skip) = (false, false, false);
        let mut note = None;
        let mut content = |ui: &mut egui::Ui| match &self.phase {
            Phase::Idle | Phase::Looking { asked: false, .. } => {}
            Phase::Looking { asked: true, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for a newer release...");
                });
            }
            Phase::Installing { version, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Updating to robocode-s {version}..."));
                });
            }
            Phase::Note(text) => {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(text.as_str()).wrap());
                    close = ui.small_button("x").on_hover_text("Dismiss").clicked();
                });
            }
            Phase::Offer(release) => {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("robocode-s {} is out", release.version)).strong(),
                    );
                    ui.label(RichText::new(format!("(this is {VERSION})")).weak());
                });
                ui.add_space(4.0);
                match self.install.how(release) {
                    How::Here { .. } => {
                        let clicked = ui
                            .add_enabled(wait.is_none(), egui::Button::new("Update and restart"))
                            .on_hover_text("Download it, check it, install it and start it")
                            .on_disabled_hover_text(wait.unwrap_or_default())
                            .clicked();
                        if clicked {
                            action = Action::Install;
                        }
                    }
                    How::Command(command) => {
                        ui.label("Update it with");
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(command).monospace());
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(command.into());
                            }
                        });
                    }
                    How::Page => {
                        if ui.button("Open the download page").clicked() {
                            note = open_url(update::DOWNLOAD_PAGE).err();
                        }
                    }
                }
                ui.horizontal(|ui| {
                    if ui.link("What's new").clicked() {
                        note = open_url(&release.notes).err();
                    }
                    skip = ui
                        .link("Skip this version")
                        .on_hover_text("Not offered again; the options can still look for it")
                        .clicked();
                    later = ui
                        .link("Later")
                        .on_hover_text("Asked again tomorrow")
                        .clicked();
                });
            }
        };
        if matches!(
            self.phase,
            Phase::Idle | Phase::Looking { asked: false, .. }
        ) {
            return Action::None;
        }
        egui::Area::new(egui::Id::new("update-notice"))
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, BAR_HEIGHT + 12.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(shell::BAR)
                    .stroke(egui::Stroke::new(1.0, shell::ACCENT))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        content(ui);
                    });
            });
        if let Phase::Offer(release) = &self.phase
            && (skip || later)
        {
            let mut state = State::load();
            if skip {
                state.skipped = Some(release.version.clone());
            }
            state.told = update::now();
            let _ = state.save();
            close = true;
        }
        if let Some(note) = note {
            self.phase = Phase::Note(note);
        } else if close {
            self.phase = Phase::Idle;
        }
        action
    }
}

/// The variable that tells the new release, started by [`restart`], which process to wait
/// for: the old one still has the IDE port.
const OLD_PID_VAR: &str = "ROBOCODES_UPDATED_FROM";

/// Starts the program `exe` (the new release), which waits for this one to exit.
pub fn restart(exe: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new(exe)
        .env(OLD_PID_VAR, std::process::id().to_string())
        .spawn()
        .map(drop)
}

/// At start: waits (up to 10 seconds) for the program that [`restart`]ed this one to exit.
pub fn wait_for_old() {
    let Some(pid) = std::env::var(OLD_PID_VAR)
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    else {
        return;
    };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while robocode_s_runner::housekeeping::process_running(pid) && std::time::Instant::now() < until
    {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// How updates come to this installation, for the options page.
pub fn describe(install: &Install) -> String {
    match install {
        Install::Source => "A build from source: it isn't updated.".into(),
        Install::Archive { root } => format!(
            "Installed from an archive in {}: updates replace its files (robots you added \
             to its robots folder stay).",
            root.display()
        ),
        Install::Bundle { app } => format!("Updates replace {}.", app.display()),
        Install::Setup { .. } => "Installed with the installer: updates run the new one.".into(),
        Install::Managed(m) => format!("Updated by your package manager: {}", m.command()),
        Install::Package => format!(
            "Installed from a package without its repository: new releases are on {}.",
            update::DOWNLOAD_PAGE
        ),
    }
}
