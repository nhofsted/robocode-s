// SPDX-License-Identifier: EPL-2.0

//! The application: the tabs (battle setup and a live battle, replays, options) in the
//! shell (see [`crate::shell`]).

use crate::backdrop::Backdrop;
use crate::icons;
use crate::live::Live;
use crate::replay::ReplayView;
use crate::settings::Settings;
use crate::shell::{self, Tab};
use crate::sprites::Sprites;
use crate::view::{self, Field, Style};
use eframe::egui::{self, Color32, RichText};
use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1;
use robocode_s_runner::local::{Environment, Listed, list_robots, prepare, random_seed};
use robocode_s_server::ServerConfig;
use robocode_s_server::results::contestants;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

/// The battle setup form.
#[derive(Clone, Debug)]
pub struct Setup {
    pub robots: String,
    pub rounds: i32,
    pub seed: String,
    pub width: i32,
    pub height: i32,
    pub classic: bool,
    pub record: String,
    pub paint: bool,
    pub turns_per_second: u32,
}

impl Setup {
    /// A new battle with the defaults from the options.
    pub fn new(settings: &Settings) -> Self {
        Self {
            robots: "sample.Fire, sample.Walls, sample.Crazy, sample.SpinBot".into(),
            rounds: settings.rounds,
            seed: String::new(),
            width: settings.width,
            height: settings.height,
            classic: settings.classic,
            record: String::new(),
            paint: false,
            turns_per_second: 30,
        }
    }
}

/// Saves a screenshot to a file once the condition holds, then quits (for checks).
pub struct Screenshot {
    pub path: PathBuf,
    /// Live: wait for this turn of the first round; otherwise this many frames.
    pub after: i64,
    requested: bool,
    frames: i64,
}

impl Screenshot {
    pub fn new(path: PathBuf, after: i64) -> Self {
        Self {
            path,
            after,
            requested: false,
            frames: 0,
        }
    }
}

pub struct App {
    sprites: Sprites,
    backdrop: Backdrop,
    env: Environment,
    setup: Setup,
    tab: Tab,
    /// The battle in the Start Battle tab; the setup when there is none.
    live: Option<Box<Live>>,
    /// The recording in the Replay Battle tab; a file field when there is none.
    replay: Option<Box<ReplayView>>,
    selected: usize,
    error: Option<String>,
    replay_error: Option<String>,
    screenshot: Option<Screenshot>,
    open_path: String,
    /// The robots in the jars, listed in the background.
    available: Option<Result<Vec<Listed>, String>>,
    listing: Option<Receiver<Result<Vec<Listed>, String>>>,
    filter: String,
    /// Draw the window frame ourselves (see [`crate::window`]).
    own_frame: bool,
    /// The options, saved on every change.
    settings: Settings,
    /// Robot jars from `ROBOCODES_ROBOT_JARS`, besides the robot folders.
    env_jars: Vec<PathBuf>,
    /// The robot jars per robot folder; `None` for a folder that isn't there.
    folder_jars: Vec<Option<usize>>,
    new_folder: String,
    settings_error: Option<String>,
}

/// How the app starts: the battle setup, a battle, a replay or another tab.
pub struct Launch {
    pub setup: Setup,
    pub start: bool,
    pub replay: Option<PathBuf>,
    pub tab: Option<Tab>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        mut env: Environment,
        settings: Settings,
        launch: Launch,
        screenshot: Option<Screenshot>,
        own_frame: bool,
    ) -> Self {
        let env_jars = crate::settings::env_jars();
        env.jars = settings.robot_jars(&env_jars);
        let mut app = Self {
            sprites: Sprites::new(&cc.egui_ctx),
            backdrop: Backdrop::default(),
            env,
            setup: launch.setup,
            tab: Tab::Start,
            live: None,
            replay: None,
            selected: 0,
            error: None,
            replay_error: None,
            screenshot,
            open_path: String::new(),
            available: None,
            listing: None,
            filter: String::new(),
            own_frame,
            folder_jars: Vec::new(),
            settings,
            env_jars,
            new_folder: String::new(),
            settings_error: None,
        };
        app.count_folder_jars();
        app.list_robots(&cc.egui_ctx);
        if let Some(path) = launch.replay {
            app.open_replay(path);
        } else if launch.start {
            app.start(&cc.egui_ctx);
        }
        if let Some(tab) = launch.tab {
            app.tab = tab;
        }
        app
    }

    /// How the battlefield is drawn, from the options.
    fn style(&self) -> Style {
        Style {
            scan_arcs: self.settings.scan_arcs,
            colored_bullets: self.settings.colored_bullets,
        }
    }

    fn count_folder_jars(&mut self) {
        self.folder_jars = self
            .settings
            .robot_folders
            .iter()
            .map(|f| {
                let one = Settings {
                    robot_folders: vec![f.clone()],
                    ..Settings::default()
                };
                (f.exists()).then(|| one.robot_jars(&[]).len())
            })
            .collect();
    }

    /// Scans the robot folders again and lists their robots.
    fn rescan(&mut self, ctx: &egui::Context) {
        self.env.jars = self.settings.robot_jars(&self.env_jars);
        self.count_folder_jars();
        self.available = None;
        self.list_robots(ctx);
    }

    /// Lists the robots in the jars on a background thread (it starts a JVM).
    fn list_robots(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let env = self.env.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let home = env.data.join("list");
            let result = env
                .host(false)
                .and_then(|host| list_robots(&host, &env.jars, &home));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.listing = Some(rx);
    }

    /// Takes the robot list once it is there.
    fn poll_listing(&mut self) {
        if let Some(rx) = &self.listing
            && let Ok(result) = rx.try_recv()
        {
            self.available = Some(result);
            self.listing = None;
        }
    }

    /// The available robots: click one to add it to the battle.
    fn robot_chooser(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Available robots").strong());
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter"));
                egui::ScrollArea::vertical()
                    .id_salt("available")
                    .max_height(260.0)
                    .min_scrolled_height(260.0)
                    .show(ui, |ui| match &self.available {
                        None => {
                            ui.spinner();
                        }
                        Some(Err(e)) => {
                            ui.colored_label(Color32::LIGHT_RED, e);
                        }
                        Some(Ok(robots)) => {
                            let filter = self.filter.to_lowercase();
                            for r in robots
                                .iter()
                                .filter(|r| r.name.to_lowercase().contains(&filter))
                            {
                                let label = if r.team {
                                    format!("{} (team)", r.name)
                                } else {
                                    r.name.clone()
                                };
                                if ui
                                    .selectable_label(false, label)
                                    .on_hover_text("add")
                                    .clicked()
                                {
                                    let robots = &mut self.setup.robots;
                                    if !robots.trim().is_empty() {
                                        robots.push_str(", ");
                                    }
                                    robots.push_str(&r.name);
                                }
                            }
                        }
                    });
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("In the battle").strong());
                let selected: Vec<String> = self
                    .setup
                    .robots
                    .split(',')
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(String::from)
                    .collect();
                let mut remove = None;
                egui::ScrollArea::vertical()
                    .id_salt("selected")
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for (i, name) in selected.iter().enumerate() {
                            ui.horizontal(|ui| {
                                if ui.small_button("x").on_hover_text("remove").clicked() {
                                    remove = Some(i);
                                }
                                ui.label(name);
                            });
                        }
                    });
                if let Some(i) = remove {
                    let mut rest = selected;
                    rest.remove(i);
                    self.setup.robots = rest.join(", ");
                }
            });
        });
    }

    fn start(&mut self, ctx: &egui::Context) {
        let s = &self.setup;
        let robots: Vec<String> = s
            .robots
            .split(',')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(String::from)
            .collect();
        let seed = match s.seed.trim() {
            "" => random_seed(),
            text => match text.parse() {
                Ok(seed) => seed,
                Err(_) => {
                    self.error = Some(format!("not a number: {text}"));
                    return;
                }
            },
        };
        let config = ServerConfig {
            rules: BattleRules {
                width: s.width,
                height: s.height,
                num_rounds: s.rounds.max(1),
                ..Default::default()
            },
            seed,
            deterministic: !s.classic,
            record: (!s.record.trim().is_empty()).then(|| PathBuf::from(s.record.trim())),
            ..Default::default()
        };
        match prepare(&self.env, &robots, config, 512) {
            Ok((host, battle)) => {
                let mut live = Live::start(ctx, host, battle);
                live.seed = seed;
                live.set_speed(s.turns_per_second);
                if s.paint {
                    for i in 0..live.robots.len() {
                        live.set_paint(i, true);
                    }
                }
                self.selected = 0;
                self.error = None;
                self.live = Some(Box::new(live));
                self.tab = Tab::Start;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn open_replay(&mut self, path: PathBuf) {
        self.tab = Tab::Replay;
        match ReplayView::open(&path) {
            Ok(view) => {
                self.selected = 0;
                self.replay_error = None;
                self.replay = Some(Box::new(view));
            }
            Err(e) => self.replay_error = Some(format!("{}: {e}", path.display())),
        }
    }

    /// A page over the backdrop (see [`shell::page`]).
    fn page(&mut self, root: &mut egui::Ui, content: impl FnOnce(&mut Self, &mut egui::Ui)) {
        shell::page(
            root,
            self,
            |app| (&mut app.backdrop, &mut app.sprites),
            content,
        );
    }

    /// The Start Battle tab without a battle: the setup.
    fn start_page(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        self.page(root, |app, ui| {
            ui.heading("Start Battle");
            ui.add_space(8.0);
            app.robot_chooser(ui);
            ui.add_space(12.0);
            let s = &mut app.setup;
            egui::Grid::new("setup")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Robots");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.robots)
                            .desired_width(480.0)
                            .hint_text("sample.Fire, sample.Walls, sampleteam.MyFirstTeam"),
                    );
                    ui.end_row();
                    ui.label("Rounds");
                    ui.add(egui::DragValue::new(&mut s.rounds).range(1..=10_000));
                    ui.end_row();
                    ui.label("Battlefield");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.width).range(400..=5000));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut s.height).range(400..=5000));
                    });
                    ui.end_row();
                    ui.label("Seed");
                    ui.add(egui::TextEdit::singleline(&mut s.seed).hint_text("random"));
                    ui.end_row();
                    ui.label("Mode");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.classic, false, "Deterministic");
                        ui.radio_value(&mut s.classic, true, "Classic timing");
                    });
                    ui.end_row();
                    ui.label("Record to");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.record)
                            .hint_text("file (optional)")
                            .desired_width(480.0),
                    );
                    ui.end_row();
                    ui.label("Debug paint");
                    ui.checkbox(&mut s.paint, "for every robot");
                    ui.end_row();
                });
            ui.add_space(12.0);
            if ui.button(RichText::new("Start battle").strong()).clicked() {
                app.start(ctx);
            }
            if let Some(e) = &app.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!(
                    "Robots come from {} jars in the robot folders (see Options){}.",
                    app.env.jars.len(),
                    if app.env_jars.is_empty() {
                        ""
                    } else {
                        " and ROBOCODES_ROBOT_JARS"
                    }
                ))
                .weak(),
            );
        });
    }

    /// The Replay Battle tab without a recording: which file to open.
    fn replay_page(&mut self, root: &mut egui::Ui) {
        self.page(root, |app, ui| {
            ui.heading("Replay Battle");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Recording");
                ui.add(
                    egui::TextEdit::singleline(&mut app.open_path)
                        .hint_text("a file from --record or \"Record to\"")
                        .desired_width(480.0),
                );
                if ui.button("Open").clicked() && !app.open_path.trim().is_empty() {
                    let path = PathBuf::from(app.open_path.trim());
                    app.open_replay(path);
                }
            });
            if let Some(e) = &app.replay_error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
        });
    }

    /// The Run Rumble tab (M5); its tab is disabled until then.
    fn rumble_page(&mut self, root: &mut egui::Ui) {
        self.page(root, |_, ui| {
            ui.heading("Run Rumble");
            ui.add_space(8.0);
            ui.label(RichText::new("Coming with the rumble client (M5).").weak());
        });
    }

    /// The options, behind the gear: robot folders, how battles are drawn, and the
    /// defaults for new battles. Every change is saved at once.
    fn options_page(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        let before = self.settings.clone();
        let mut rescan = false;
        self.page(root, |app, ui| {
            ui.heading("Options");
            ui.add_space(4.0);
            match Settings::path() {
                Some(path) => ui.label(
                    RichText::new(format!("Saved in {}", path.display()))
                        .small()
                        .weak(),
                ),
                None => ui.label(RichText::new("Not saved: no config directory").small()),
            };

            section(ui, "Robots");
            egui::Grid::new("robot-folders")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    // One row per folder, the label on the first.
                    let mut remove = None;
                    for (i, folder) in app.settings.robot_folders.iter().enumerate() {
                        ui.label(if i == 0 { "Robot folders" } else { "" });
                        ui.horizontal(|ui| {
                            let hint = "Remove this folder";
                            if icons::button(ui, 20.0, hint, false, |p, c, color| {
                                icons::cross(p, c, 4.0, color);
                            })
                            .clicked()
                            {
                                remove = Some(i);
                            }
                            ui.label(RichText::new(folder.display().to_string()).monospace());
                            match app.folder_jars.get(i).copied().flatten() {
                                Some(1) => ui.label(RichText::new("1 jar").weak()),
                                Some(n) => ui.label(RichText::new(format!("{n} jars")).weak()),
                                None => ui.colored_label(Color32::LIGHT_RED, "not found"),
                            };
                        });
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        app.settings.robot_folders.remove(i);
                    }
                    if app.settings.robot_folders.is_empty() {
                        ui.label("Robot folders");
                        ui.label(RichText::new("None yet: add a folder with robot jars.").weak());
                        ui.end_row();
                    }
                    ui.label("");
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut app.new_folder)
                                .hint_text("a folder with robot jars, e.g. ~/robocode/robots")
                                .desired_width(420.0),
                        );
                        let enter =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let text = app.new_folder.trim();
                        let add = ui.add_enabled(!text.is_empty(), egui::Button::new("Add"));
                        if (add.clicked() || enter) && !text.is_empty() {
                            let folder = expand_home(text);
                            if !app.settings.robot_folders.contains(&folder) {
                                app.settings.robot_folders.push(folder);
                            }
                            app.new_folder.clear();
                        }
                    });
                    ui.end_row();

                    ui.label("Found");
                    ui.horizontal(|ui| {
                        let robots = match &app.available {
                            None => "listing robots…".to_string(),
                            Some(Ok(robots)) => format!("{} robots", robots.len()),
                            Some(Err(_)) => "the robots couldn't be listed".to_string(),
                        };
                        ui.label(format!("{} jars, {robots}", app.env.jars.len()));
                        if app.listing.is_some() {
                            ui.spinner();
                        }
                        if ui
                            .button("Rescan")
                            .on_hover_text("Look for new jars in the folders")
                            .clicked()
                        {
                            rescan = true;
                        }
                    });
                    ui.end_row();
                    if let Some(Err(e)) = &app.available {
                        ui.label("");
                        ui.colored_label(Color32::LIGHT_RED, e);
                        ui.end_row();
                    }
                    if !app.env_jars.is_empty() {
                        ui.label("Also");
                        ui.label(
                            RichText::new(format!(
                                "{} jars from ROBOCODES_ROBOT_JARS",
                                app.env_jars.len()
                            ))
                            .weak(),
                        );
                        ui.end_row();
                    }
                });

            section(ui, "Battlefield");
            let s = &mut app.settings;
            egui::Grid::new("view-options")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Scan arcs");
                    ui.checkbox(&mut s.scan_arcs, "Show each robot's radar sweep");
                    ui.end_row();
                    ui.label("Coloured bullets");
                    ui.checkbox(
                        &mut s.colored_bullets,
                        "Bullets in their robot's bullet colour",
                    );
                    ui.end_row();
                });

            section(ui, "New battles");
            egui::Grid::new("battle-defaults")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Mode");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.classic, false, "Deterministic");
                        ui.radio_value(&mut s.classic, true, "Classic timing");
                    });
                    ui.end_row();
                    ui.label("Rounds");
                    ui.add(egui::DragValue::new(&mut s.rounds).range(1..=10_000));
                    ui.end_row();
                    ui.label("Battlefield size");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.width).range(400..=5000));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut s.height).range(400..=5000));
                    });
                    ui.end_row();
                });

            if let Some(e) = &app.settings_error {
                ui.add_space(12.0);
                ui.colored_label(
                    Color32::LIGHT_RED,
                    format!("Couldn't save the options: {e}"),
                );
            }
        });
        if self.settings != before {
            self.settings_changed(ctx, &before);
        } else if rescan {
            self.rescan(ctx);
        }
    }

    /// Saves the options and applies them: new robot folders are scanned, and new
    /// defaults go into the battle setup.
    fn settings_changed(&mut self, ctx: &egui::Context, before: &Settings) {
        self.settings_error = self.settings.save().err();
        let (s, setup) = (&self.settings, &mut self.setup);
        if s.classic != before.classic {
            setup.classic = s.classic;
        }
        if s.rounds != before.rounds {
            setup.rounds = s.rounds;
        }
        if (s.width, s.height) != (before.width, before.height) {
            (setup.width, setup.height) = (s.width, s.height);
        }
        if s.robot_folders != before.robot_folders {
            self.rescan(ctx);
        }
    }

    /// The Start Battle tab with a battle: controls, robots, console and the battlefield.
    /// True when the user goes back to the setup.
    fn live_ui(&mut self, root: &mut egui::Ui) -> bool {
        let ctx = &root.ctx().clone();
        let mut back = false;
        let style = self.style();
        let Some(live) = &mut self.live else {
            return true;
        };
        egui::Panel::top("controls").show(root, |ui| {
            ui.horizontal(|ui| {
                let label = if live.paused { "Resume" } else { "Pause" };
                if ui.button(label).clicked() {
                    let paused = !live.paused;
                    live.set_paused(paused);
                }
                if ui
                    .add_enabled(live.paused, egui::Button::new("Step"))
                    .clicked()
                {
                    live.step();
                }
                let mut tps = live.turns_per_second;
                let max = tps == 0;
                ui.label("Turns/s");
                let slider = ui.add_enabled(
                    !max,
                    egui::Slider::new(&mut tps, 1..=1000).logarithmic(true),
                );
                let mut fastest = max;
                let toggled = ui.checkbox(&mut fastest, "max").changed();
                if slider.changed() || toggled {
                    live.set_speed(if fastest { 0 } else { tps.max(1) });
                }
                ui.separator();
                if let Some(s) = &live.latest {
                    ui.label(format!(
                        "Round {}/{} · turn {}",
                        s.round + 1,
                        live.rounds,
                        s.turn
                    ));
                }
                ui.label(RichText::new(format!("seed {}", live.seed)).weak());
                if live.outcome.is_some() && ui.button("New battle").clicked() {
                    back = true;
                }
            });
        });
        egui::Panel::right("robots")
            .default_size(260.0)
            .show(root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let paint = live.paint.clone();
                    let (robots, latest) = (live.robots.clone(), live.latest.clone());
                    robot_list(
                        ui,
                        &robots,
                        latest.as_ref(),
                        &mut self.selected,
                        Some((&paint, &mut |i, on| live.set_paint(i, on))),
                    );
                });
            });
        egui::Panel::bottom("console")
            .resizable(true)
            .default_size(140.0)
            .show(root, |ui| {
                let i = self.selected.min(live.robots.len().saturating_sub(1));
                let title = live.robots.get(i).map_or("", |r| r.name.as_str());
                console(ui, title, live.output.get(i).map_or("", String::as_str));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(root, |ui| {
                let field = Field {
                    width: live.width,
                    height: live.height,
                    robots: &live.robots,
                };
                view::draw(
                    ui,
                    &mut self.sprites,
                    &field,
                    style,
                    (live.previous.as_ref(), live.latest.as_ref()),
                    ui.max_rect(),
                );
            });
        match &live.outcome {
            Some(Ok(results)) => results_window(ctx, &live.robots, results),
            Some(Err(e)) => {
                egui::Window::new("Battle failed").show(ctx, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, e);
                    if ui.button("Back").clicked() {
                        back = true;
                    }
                });
            }
            None => {}
        }
        back
    }
}

/// The width of the label column on the options page.
const LABELS: f32 = 150.0;

/// A section heading on a page: a title over a line.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(20.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .size(13.0)
            .strong()
            .color(shell::ACCENT),
    );
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, shell::LINE),
    );
}

/// A typed folder: `~` at the start is the home directory.
fn expand_home(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(home).join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(text),
    }
}

/// Debug paint toggles: which robots paint, and what to call on a change.
type PaintToggles<'a> = Option<(&'a [bool], &'a mut dyn FnMut(usize, bool))>;

/// The robot list: energy, score, debug paint toggles; a click selects a robot's console.
fn robot_list(
    ui: &mut egui::Ui,
    robots: &[v1::RobotStatics],
    snapshot: Option<&v1::TurnSnapshot>,
    selected: &mut usize,
    mut paint: PaintToggles,
) {
    for (i, statics) in robots.iter().enumerate() {
        let robot = snapshot.and_then(|s| s.robots.get(i));
        let energy = robot.and_then(|r| r.energy).unwrap_or_default();
        let dead = robot.is_some_and(|r| r.state == v1::RobotState::Dead as i32);
        ui.horizontal(|ui| {
            let name = RichText::new(&statics.name);
            let name = if dead { name.weak() } else { name };
            if ui.selectable_label(*selected == i, name).clicked() {
                *selected = i;
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::ProgressBar::new((energy / 100.0).clamp(0.0, 1.0) as f32)
                    .desired_width(120.0)
                    .text(format!("{energy:.1}")),
            );
            ui.label(format!(
                "{:.0}",
                robot.map_or(0.0, |r| r.total_score.unwrap_or_default()
                    + r.score.unwrap_or_default())
            ));
            if let Some((enabled, toggle)) = paint.as_mut() {
                let mut on = enabled[i];
                if ui.checkbox(&mut on, "paint").changed() {
                    toggle(i, on);
                }
            }
        });
        ui.add_space(4.0);
    }
}

fn console(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.label(RichText::new(title).strong());
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
        });
}

fn results_window(ctx: &egui::Context, robots: &[v1::RobotStatics], results: &[v1::Results]) {
    egui::Window::new("Results")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let rows = contestants(robots, results);
            let total: f64 = rows.iter().map(|c| c.score).sum();
            egui::Grid::new("results").striped(true).show(ui, |ui| {
                for h in [
                    "Rank",
                    "Robot",
                    "Score",
                    "Survival",
                    "Bullet dmg",
                    "Ram dmg",
                    "1sts",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for c in &rows {
                    ui.label(c.rank.to_string());
                    ui.label(&c.name);
                    let pct = if total > 0.0 {
                        100.0 * c.score / total
                    } else {
                        0.0
                    };
                    ui.label(format!("{:.0} ({pct:.0}%)", c.score));
                    ui.label(format!("{:.0}", c.survival));
                    ui.label(format!("{:.0}", c.bullet_damage));
                    ui.label(format!("{:.0}", c.ram_damage));
                    ui.label(c.firsts.to_string());
                    ui.end_row();
                }
            });
        });
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        shell::top_bar(root, self.own_frame, &mut self.sprites, &mut self.tab);
        shell::footer(root);
        self.poll_listing();
        let style = self.style();
        // A battle keeps running in its tab while another one is open.
        if let Some(live) = &mut self.live {
            live.poll();
        }
        match self.tab {
            Tab::Start if self.live.is_some() => {
                if self.live_ui(root) {
                    self.live = None;
                }
            }
            Tab::Start => self.start_page(root),
            Tab::Replay => match &mut self.replay {
                Some(replay) => {
                    if replay.ui(root, &mut self.sprites, style, &mut self.selected) {
                        self.replay = None;
                    }
                }
                None => self.replay_page(root),
            },
            Tab::Rumble => self.rumble_page(root),
            Tab::Options => self.options_page(root),
        }
        self.screenshot_step(ctx);
    }
}

impl App {
    /// `--screenshot`: once the battle is far enough (or after some frames), saves the
    /// window to a PNG and quits.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        if shot.requested {
            let image = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Err(e) = image::save_buffer(
                    &shot.path,
                    &rgba,
                    w as u32,
                    h as u32,
                    image::ExtendedColorType::Rgba8,
                ) {
                    eprintln!("screenshot: {e}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
            return;
        }
        shot.frames += 1;
        let ready = match (&self.live, self.tab) {
            (Some(live), Tab::Start) => {
                live.outcome.is_some()
                    || live
                        .latest
                        .as_ref()
                        .is_some_and(|s| s.turn >= shot.after || s.round > 0)
            }
            _ => shot.frames >= shot.after,
        };
        if ready {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
    }
}
