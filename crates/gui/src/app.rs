// SPDX-License-Identifier: EPL-2.0

//! The application: the tabs (battle setup and a live battle, replays, options) in the
//! shell (see [`crate::shell`]).

use crate::backdrop::Backdrop;
use crate::icons;
use crate::live::Live;
use crate::replay::ReplayView;
use crate::screen;
use crate::settings::{Contents, Rules, Settings};
use crate::shell::{self, Tab};
use crate::sprites::{Part, Sprites};
use crate::view::{self, Field, Style};
use eframe::egui::{self, Color32, RichText};
use egui_file_dialog::{DialogState, FileDialog, OpeningMode};
use robocode_s_engine::BattleRules;
use robocode_s_runner::JavaHost;
use robocode_s_runner::local::{
    Environment, Listed, LocalBattle, list_robots, prepare, random_seed,
};
use robocode_s_server::{BattleOutcome, ServerConfig, battle_file};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

/// The battle setup form.
#[derive(Clone, Debug)]
pub struct Setup {
    /// The robots and teams in the battle, by the names they are listed by; one may be in
    /// it more than once, as in classic.
    pub robots: Vec<String>,
    pub rounds: i32,
    pub seed: String,
    pub width: i32,
    pub height: i32,
    pub classic: bool,
    pub record: String,
    pub paint: bool,
    pub turns_per_second: u32,
    pub rules: Rules,
}

impl Setup {
    /// A new battle with the defaults from the options.
    pub fn new(settings: &Settings) -> Self {
        Self {
            robots: parse_robots("sample.Fire, sample.Walls, sample.Crazy, sample.SpinBot"),
            rounds: settings.rounds,
            seed: String::new(),
            width: settings.width,
            height: settings.height,
            classic: settings.classic,
            record: String::new(),
            paint: false,
            turns_per_second: 30,
            rules: settings.rules,
        }
    }

    /// The setup's rules, as the engine takes them.
    pub fn battle_rules(&self) -> BattleRules {
        BattleRules {
            width: self.width,
            height: self.height,
            num_rounds: self.rounds.max(1),
            gun_cooling_rate: self.rules.gun_cooling_rate,
            inactivity_time: self.rules.inactivity_time,
            hide_enemy_names: self.rules.hide_enemy_names,
            sentry_border_size: self.rules.sentry_border,
        }
    }

    /// Takes the rules of a `.battle` file, and its robots if it names any.
    pub fn open(&mut self, file: battle_file::BattleFile) {
        let r = file.rules;
        (self.width, self.height, self.rounds) = (r.width, r.height, r.num_rounds.max(1));
        self.rules = Rules {
            gun_cooling_rate: r.gun_cooling_rate,
            inactivity_time: r.inactivity_time,
            hide_enemy_names: r.hide_enemy_names,
            sentry_border: r.sentry_border_size,
        };
        if !file.robots.is_empty() {
            self.robots = file.robots;
        }
    }
}

/// A game type (Tank Royale's): its name, a hint, and the rounds, battlefield size and
/// rules it sets.
pub struct GameType {
    pub name: &'static str,
    pub hint: &'static str,
    pub rounds: i32,
    pub size: (i32, i32),
}

/// The game types: classic's defaults, and the rumbles' 1v1, melee and team settings.
/// Each sets the default rules.
pub const GAME_TYPES: [GameType; 4] = [
    GameType {
        name: "Classic",
        hint: "Classic Robocode's defaults: 10 rounds on 800 × 600",
        rounds: 10,
        size: (800, 600),
    },
    GameType {
        name: "1v1",
        hint: "Two robots, as the RoboRumble: 35 rounds on 800 × 600",
        rounds: 35,
        size: (800, 600),
    },
    GameType {
        name: "Melee",
        hint: "Many robots, as the MeleeRumble: 35 rounds on 1000 × 1000",
        rounds: 35,
        size: (1000, 1000),
    },
    GameType {
        name: "Teams",
        hint: "Teams, as the TeamRumble: 10 rounds on 1200 × 1200",
        rounds: 10,
        size: (1200, 1200),
    },
];

impl GameType {
    /// Whether `setup` has this game type's settings.
    pub fn matches(&self, setup: &Setup) -> bool {
        setup.rounds == self.rounds
            && (setup.width, setup.height) == self.size
            && setup.rules == Rules::default()
    }

    pub fn apply(&self, setup: &mut Setup) {
        setup.rounds = self.rounds;
        (setup.width, setup.height) = self.size;
        setup.rules = Rules::default();
    }
}

/// `--battle`: robots and teams separated by commas.
pub fn parse_robots(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(String::from)
        .collect()
}

/// A battle on its way, shown on the loading screen: the robots are described on a
/// background thread, then their JVMs start and connect; it ends with the first snapshot.
struct Starting {
    /// The contestants: the selection at first, the robots in battle order once they are
    /// described.
    contestants: Vec<String>,
    /// The described battle, until it is there.
    prepared: Option<Receiver<Result<(JavaHost, LocalBattle), String>>>,
    seed: i64,
    rounds: i32,
    size: (i32, i32),
    classic: bool,
    turns_per_second: u32,
    paint: bool,
    /// Why the battle couldn't start.
    error: Option<String>,
}

/// Saves a screenshot to a file once the condition holds, then quits (for checks).
pub struct Screenshot {
    pub path: PathBuf,
    /// Live: wait for this turn of the first round; otherwise this many frames.
    pub after: i64,
    /// Shoot the loading screen of a battle (after `after` frames) rather than the battle.
    pub loading: bool,
    requested: bool,
    frames: i64,
    /// Frames since the battle ended, for its results window to be laid out.
    ended: i64,
}

impl Screenshot {
    pub fn new(path: PathBuf, after: i64, loading: bool) -> Self {
        Self {
            path,
            after,
            loading,
            requested: false,
            frames: 0,
            ended: 0,
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
    /// The battle being started: the Start Battle tab shows the loading screen.
    starting: Option<Starting>,
    /// The recording in the Replay Battle tab; a file field when there is none.
    replay: Option<Box<ReplayView>>,
    selected: usize,
    error: Option<String>,
    replay_error: Option<String>,
    screenshot: Option<Screenshot>,
    /// `--kill-at`, until done.
    kill_at: Option<(i64, usize)>,
    /// `--input-at`, by turn, until sent.
    input_at: Vec<(i64, Vec<robocode_s_protocol::v1::InputEvent>)>,
    /// How battles and replays first show the battlefield (`--zoom`; fitted by default).
    camera: view::Camera,
    open_path: String,
    /// The robots in the jars and class folders, listed in the background.
    available: Option<Result<Vec<Listed>, String>>,
    listing: Option<Receiver<Result<Vec<Listed>, String>>>,
    /// The robot whose details show below the robot lists: the last one clicked.
    details: Option<String>,
    filter: String,
    /// The highlighted robot in the available list (by name) and in the battle (by index).
    picked: Option<String>,
    picked_selected: Option<usize>,
    /// Draw the window frame ourselves (see [`crate::window`]).
    own_frame: bool,
    /// The options, saved on every change.
    settings: Settings,
    /// Robot jars and class folders from `ROBOCODES_ROBOT_JARS`, besides the robot folders.
    env_jars: Vec<PathBuf>,
    /// What each robot folder holds; `None` for a folder that isn't there.
    folder_contents: Vec<Option<Contents>>,
    new_folder: String,
    settings_error: Option<String>,
    /// The battles started, so each one's results window opens centred.
    battles: u64,
    /// The file dialog behind the fields' folder buttons (drawn by egui: WSLg has no
    /// native one), and the field it is open for. It opens where the last pick was.
    files: FileDialog,
    browsing: Option<Browse>,
    /// The text [`Browse::Save`] saves, and why the last save failed.
    saving: Option<screen::SaveRequest>,
    save_error: Option<String>,
}

/// The field a file dialog is open for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Browse {
    /// Start Battle's "Record to": a file to save.
    Record,
    /// Replay Battle's recording: a file to open.
    Replay,
    /// A robot folder for the options.
    Folder,
    /// A file to save [`App::saving`] to.
    Save,
    /// A classic `.battle` file to open into the battle setup.
    OpenBattle,
}

/// What the user wants from the battle screen.
enum Next {
    /// The battle stays.
    Stay,
    /// Back to the setup, with the battle's selection.
    Setup,
    /// The same battle again: with its seed (so that it repeats exactly in deterministic
    /// mode) or with a new one.
    Restart { new_seed: bool },
}

/// How the app starts: the battle setup, a battle, a replay or another tab.
pub struct Launch {
    pub setup: Setup,
    pub start: bool,
    pub replay: Option<PathBuf>,
    pub tab: Option<Tab>,
    /// The field whose file dialog opens at start (`--browse`).
    pub browse: Option<Browse>,
    /// Kill a robot at a turn of the first round, as its kill button would (`--kill-at`,
    /// for screenshots and testing): (turn, robot index).
    pub kill_at: Option<(i64, usize)>,
    /// Mouse and keyboard events to send at turns of the first round, as the battlefield
    /// would (`--input-at`, for screenshots and testing).
    pub input_at: Vec<(i64, Vec<robocode_s_protocol::v1::InputEvent>)>,
    /// How battles and replays first show the battlefield (`--zoom`, for screenshots).
    pub camera: view::Camera,
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
        env.jars = settings.robot_paths(&env_jars);
        let mut app = Self {
            sprites: Sprites::new(&cc.egui_ctx),
            backdrop: Backdrop::default(),
            env,
            setup: launch.setup,
            tab: Tab::Start,
            live: None,
            starting: None,
            replay: None,
            selected: 0,
            error: None,
            replay_error: None,
            screenshot,
            kill_at: launch.kill_at,
            input_at: launch.input_at,
            camera: launch.camera,
            open_path: String::new(),
            available: None,
            listing: None,
            details: None,
            filter: String::new(),
            picked: None,
            picked_selected: None,
            own_frame,
            folder_contents: Vec::new(),
            settings,
            env_jars,
            new_folder: String::new(),
            settings_error: None,
            battles: 0,
            files: FileDialog::new()
                .id("file-dialog")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]),
            browsing: None,
            saving: None,
            save_error: None,
        };
        app.scan_folders();
        app.list_robots(&cc.egui_ctx);
        if let Some(path) = launch.replay {
            app.open_replay(path);
        } else if launch.start {
            app.start(&cc.egui_ctx);
        }
        if let Some(tab) = launch.tab {
            app.tab = tab;
        }
        if let Some(field) = launch.browse {
            app.browse(field);
        }
        app
    }

    /// How the battlefield is drawn, from the options.
    fn style(&self) -> Style {
        Style {
            scan_arcs: self.settings.scan_arcs,
            colored_bullets: self.settings.colored_bullets,
            robot_names: self.settings.robot_names,
            robot_energy: self.settings.robot_energy,
            explosions: self.settings.explosions,
            ground: self.settings.ground,
            sentry_border: self.settings.sentry_border,
            rates: self.settings.rates,
        }
    }

    /// Looks at what the robot folders hold.
    fn scan_folders(&mut self) {
        self.env.jars = self.settings.robot_paths(&self.env_jars);
        self.folder_contents = self
            .settings
            .robot_folders
            .iter()
            .map(|f| f.exists().then(|| Contents::of(f)))
            .collect();
    }

    /// Scans the robot folders again and lists their robots.
    fn rescan(&mut self, ctx: &egui::Context) {
        self.scan_folders();
        self.available = None;
        self.list_robots(ctx);
    }

    /// Lists the robots again when the battle setup shows (new robots compiled into a
    /// class folder, a jar dropped into a robot folder), keeping the current list until
    /// the new one is there. A recompiled robot needs no new list: every battle loads its
    /// robots afresh.
    fn refresh(&mut self, ctx: &egui::Context) {
        if self.listing.is_none() {
            self.scan_folders();
            self.list_robots(ctx);
        }
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

    /// The robots: the available ones with their filter on the left, the battle's on the
    /// right. A double-click (or the row's plus) adds a robot, again for another instance;
    /// a double-click (or the row's cross) removes it.
    fn robot_chooser(&mut self, ui: &mut egui::Ui) {
        const GAP: f32 = 24.0;
        let width = ((ui.available_width() - GAP) / 2.0).max(160.0);
        // The lists take what the window has left over the rest of the page, within limits.
        let lists = (ui.ctx().content_rect().height() - 530.0).clamp(96.0, 300.0);
        let column = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(width);
                    add(ui);
                },
            );
        };
        let mut add = None;
        let mut field_height = ui.spacing().interact_size.y;
        ui.horizontal_top(|ui| {
            column(ui, &mut |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Filter the robots")
                        .desired_width(width),
                );
                field_height = field.rect.height();
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                list_box(ui, "available", lists, |ui| match &self.available {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Listing the robots…").weak());
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Color32::LIGHT_RED, e);
                    }
                    Some(Ok(robots)) => {
                        let filter = self.filter.to_lowercase();
                        let shown: Vec<&Listed> = robots
                            .iter()
                            .filter(|r| r.name.to_lowercase().contains(&filter))
                            .collect();
                        if shown.is_empty() {
                            let none = if robots.is_empty() {
                                "No robots: add a robot folder in the options."
                            } else {
                                "No robot matches the filter."
                            };
                            ui.label(RichText::new(none).weak());
                        }
                        for r in &shown {
                            let picked = self.picked.as_deref() == Some(r.name.as_str());
                            let row = robot_row(ui, &r.name, r.team, picked, Action::Add);
                            if row.clicked {
                                self.picked = Some(r.name.clone());
                                self.details = Some(r.name.clone());
                            }
                            if row.act {
                                add = Some(r.name.clone());
                            }
                        }
                        if enter {
                            // Enter adds the highlighted robot, else the first match.
                            let picked = shown
                                .iter()
                                .find(|r| self.picked.as_deref() == Some(r.name.as_str()))
                                .or(shown.first());
                            add = picked.map(|r| r.name.clone());
                            field.request_focus();
                        }
                    }
                });
            });
            ui.add_space(GAP);
            column(ui, &mut |ui| {
                let robots = &mut self.setup.robots;
                let height = field_height;
                ui.allocate_ui_with_layout(
                    egui::vec2(width, height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(height);
                        let count = match robots.len() {
                            0 => "In the battle".to_string(),
                            n => format!("In the battle ({n})"),
                        };
                        ui.label(RichText::new(count).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(!robots.is_empty(), egui::Button::new("Clear"))
                                .clicked()
                            {
                                robots.clear();
                                self.picked_selected = None;
                            }
                        });
                    },
                );
                ui.add_space(6.0);
                let mut remove = None;
                list_box(ui, "selected", lists, |ui| {
                    if robots.is_empty() {
                        ui.label(RichText::new("Double-click a robot to add it.").weak());
                    }
                    for (i, name) in robots.iter().enumerate() {
                        let team = matches!(&self.available,
                            Some(Ok(a)) if a.iter().any(|r| r.team && &r.name == name));
                        let picked = self.picked_selected == Some(i);
                        let row = robot_row(ui, name, team, picked, Action::Remove);
                        if row.clicked {
                            self.picked_selected = Some(i);
                            self.details = Some(name.clone());
                        }
                        if row.act {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    robots.remove(i);
                    self.picked_selected = None;
                }
            });
        });
        if let Some(name) = add {
            self.setup.robots.push(name);
        }
        self.robot_details(ui);
    }

    /// The details of the robot clicked last, from its `.properties`: its name and
    /// version, author, description and web page.
    fn robot_details(&self, ui: &mut egui::Ui) {
        let Some(Ok(available)) = &self.available else {
            return;
        };
        let Some(robot) = self
            .details
            .as_ref()
            .and_then(|name| available.iter().find(|r| &r.name == name))
        else {
            return;
        };
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&robot.name).strong());
            if !robot.author.is_empty() {
                ui.label(RichText::new(format!("by {}", robot.author)).weak());
            }
            if !robot.webpage.is_empty() {
                ui.hyperlink_to(&robot.webpage, &robot.webpage);
            }
        });
        let description = if robot.description.is_empty() {
            RichText::new("No description.").weak().italics()
        } else {
            RichText::new(&robot.description).weak()
        };
        ui.add(egui::Label::new(description).wrap());
    }

    /// Starts the battle in the setup: the loading screen while the robots are described
    /// (on a background thread) and their JVMs start.
    fn start(&mut self, ctx: &egui::Context) {
        if self.setup.robots.is_empty() {
            self.error = Some("Add some robots to the battle first.".into());
            return;
        }
        let seed = match self.setup.seed.trim() {
            "" => random_seed(),
            text => match text.parse() {
                Ok(seed) => seed,
                Err(_) => {
                    self.error = Some(format!("The seed is not a number: {text}"));
                    return;
                }
            },
        };
        self.launch(ctx, seed);
    }

    /// Starts the battle in the setup with `seed` (see [`App::start`]).
    fn launch(&mut self, ctx: &egui::Context, seed: i64) {
        let s = &self.setup;
        let config = ServerConfig {
            rules: s.battle_rules(),
            seed,
            deterministic: !s.classic,
            record: (!s.record.trim().is_empty()).then(|| PathBuf::from(s.record.trim())),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        let (env, robots, repaint) = (self.env.clone(), s.robots.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(prepare(&env, &robots, config, 512));
            repaint.request_repaint();
        });
        self.starting = Some(Starting {
            contestants: s.robots.clone(),
            prepared: Some(rx),
            seed,
            rounds: s.rounds.max(1),
            size: (s.width, s.height),
            classic: s.classic,
            turns_per_second: s.turns_per_second,
            paint: s.paint,
            error: None,
        });
        self.error = None;
        self.live = None;
        self.battles += 1;
        self.tab = Tab::Start;
    }

    /// Moves a starting battle along: once it is described the battle starts, and with its
    /// first snapshot (or its end) the loading screen gives way to it.
    fn poll_starting(&mut self, ctx: &egui::Context) {
        let Some(starting) = &mut self.starting else {
            return;
        };
        if let Some(rx) = &starting.prepared
            && let Ok(result) = rx.try_recv()
        {
            starting.prepared = None;
            match result {
                Ok((host, battle)) => {
                    starting.contestants = battle
                        .robots
                        .iter()
                        .map(|r| r.statics.name.clone())
                        .collect();
                    let mut live = Live::start(ctx, host, battle);
                    live.camera = self.camera;
                    live.seed = starting.seed;
                    live.set_speed(starting.turns_per_second);
                    if starting.paint {
                        for i in 0..live.robots.len() {
                            live.set_paint(i, true);
                        }
                    }
                    self.selected = 0;
                    self.live = Some(Box::new(live));
                }
                Err(e) => starting.error = Some(e),
            }
        }
        if let Some(live) = &self.live {
            match &live.outcome {
                // It failed before its first turn: say so on the loading screen.
                Some(Err(e)) if live.latest.is_none() => {
                    starting.error = Some(e.clone());
                    self.live = None;
                }
                _ if live.latest.is_some() || live.outcome.is_some() => self.starting = None,
                _ => {}
            }
        }
    }

    fn open_replay(&mut self, path: PathBuf) {
        self.tab = Tab::Replay;
        match ReplayView::open(&path) {
            Ok(mut view) => {
                self.selected = 0;
                self.replay_error = None;
                view.camera = self.camera;
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
            section(ui, "Robots");
            app.robot_chooser(ui);

            section(ui, "Battle");
            let mut browse = None;
            let mut save_battle = false;
            let s = &mut app.setup;
            egui::Grid::new("setup")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Game type");
                    ui.horizontal(|ui| {
                        let mut custom = true;
                        for game in &GAME_TYPES {
                            let on = game.matches(s);
                            custom &= !on;
                            if ui
                                .selectable_label(on, game.name)
                                .on_hover_text(game.hint)
                                .clicked()
                            {
                                game.apply(s);
                            }
                        }
                        ui.add_enabled(false, egui::Button::selectable(custom, "Custom"))
                            .on_disabled_hover_text("Rules of your own: change any below");
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
                    rules_rows(ui, &mut s.rules);
                    ui.label("Seed");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.seed)
                            .hint_text("random")
                            .desired_width(160.0),
                    );
                    ui.end_row();
                    ui.label("Mode");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.classic, false, "Deterministic");
                        ui.radio_value(&mut s.classic, true, "Classic timing");
                    });
                    ui.end_row();
                    ui.label("Record to");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut s.record)
                                .hint_text("a file (optional)")
                                .desired_width((ui.available_width() - 40.0).clamp(120.0, 360.0)),
                        );
                        if browse_button(ui, "Choose the file to record to") {
                            browse = Some(Browse::Record);
                        }
                    });
                    ui.end_row();
                    ui.label("Debug paint");
                    ui.checkbox(&mut s.paint, "On for every robot");
                    ui.end_row();
                    ui.label("Battle file");
                    ui.horizontal(|ui| {
                        if ui
                            .button("Open…")
                            .on_hover_text("The robots and rules from a classic .battle file")
                            .clicked()
                        {
                            browse = Some(Browse::OpenBattle);
                        }
                        save_battle = ui
                            .button("Save…")
                            .on_hover_text("The robots and rules as a classic .battle file")
                            .clicked();
                    });
                    ui.end_row();
                });
            if let Some(field) = browse {
                app.browse(field);
            }
            if save_battle {
                app.save_battle();
            }

            ui.add_space(20.0);
            ui.horizontal(|ui| {
                let jars = format!(
                    "Robots from {} in the robot folders (see Options){}.",
                    sources(&app.env.jars),
                    if app.env_jars.is_empty() {
                        ""
                    } else {
                        " and ROBOCODES_ROBOT_JARS"
                    }
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let start = egui::Button::new(
                        RichText::new("Start Battle")
                            .size(16.0)
                            .strong()
                            .color(shell::BAR),
                    )
                    .fill(shell::ACCENT)
                    .min_size(egui::vec2(170.0, 40.0));
                    let ready = !app.setup.robots.is_empty();
                    if ui.add_enabled(ready, start).clicked() {
                        app.start(ctx);
                    }
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        match &app.error {
                            Some(e) => ui.colored_label(Color32::LIGHT_RED, e),
                            None => ui.label(RichText::new(jars).weak()),
                        };
                    });
                });
            });
        });
    }

    /// The Start Battle tab while a battle starts: the contestants, and what is going on
    /// with a way to cancel (or what went wrong, with the way back to the setup).
    fn loading_page(&mut self, root: &mut egui::Ui) {
        let time = root.input(|i| i.time) as f32;
        let mut back = false;
        self.page(root, |app, ui| {
            let Some(starting) = &app.starting else {
                return;
            };
            section(ui, "Starting the battle");
            ui.add_space(8.0);
            contestants_row(ui, &mut app.sprites, &starting.contestants, time);
            ui.add_space(12.0);
            let (w, h) = starting.size;
            let mode = if starting.classic {
                "classic timing"
            } else {
                "deterministic"
            };
            let rounds = match starting.rounds {
                1 => "1 round".to_string(),
                n => format!("{n} rounds"),
            };
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{rounds} · {w} × {h} · {mode} · seed {}",
                        starting.seed
                    ))
                    .weak(),
                );
                ui.add_space(20.0);
                match &starting.error {
                    None => {
                        let what = if starting.prepared.is_some() {
                            "Looking up the robots…"
                        } else {
                            "Starting the robots…"
                        };
                        ui.add(egui::Spinner::new().size(28.0).color(shell::ACCENT));
                        ui.add_space(8.0);
                        ui.label(what);
                        ui.add_space(16.0);
                        let button = egui::Button::new("Cancel").min_size(egui::vec2(120.0, 30.0));
                        if ui.add(button).clicked() {
                            back = true;
                        }
                    }
                    Some(e) => {
                        ui.label(
                            RichText::new("The battle couldn't start")
                                .size(16.0)
                                .strong()
                                .color(Color32::LIGHT_RED),
                        );
                        ui.add_space(8.0);
                        egui::Frame::NONE
                            .fill(shell::BAR)
                            .stroke(egui::Stroke::new(1.0, shell::LINE))
                            .corner_radius(4.0)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.add(egui::Label::new(RichText::new(e).monospace()).wrap());
                                });
                            });
                        ui.add_space(16.0);
                        let button = egui::Button::new(RichText::new("Back to the setup").strong())
                            .min_size(egui::vec2(170.0, 32.0));
                        if ui.add(button).clicked() {
                            back = true;
                        }
                    }
                }
                ui.add_space(8.0);
            });
        });
        if back {
            // A battle that is already starting stops (see `Live`'s drop); robots still
            // being described are left to finish, and dropped.
            self.starting = None;
            self.live = None;
        }
    }

    /// Opens the file dialog for `field`, starting from the field's folder if it has one
    /// (otherwise where the last pick was).
    pub fn browse(&mut self, field: Browse) {
        let text = match field {
            Browse::Record => &self.setup.record,
            Browse::Replay => &self.open_path,
            Browse::Folder => &self.new_folder,
            Browse::Save | Browse::OpenBattle => "",
        };
        let path = expand_home(text.trim());
        let dir = match field {
            Browse::Folder => Some(path.as_path()),
            _ => path.parent(),
        };
        let config = self.files.config_mut();
        config.opening_mode = match dir.filter(|d| d.is_dir()) {
            Some(dir) => {
                config.initial_directory = dir.to_path_buf();
                OpeningMode::AlwaysInitialDir
            }
            None => OpeningMode::LastPickedDir,
        };
        let saving = self.saving.as_ref();
        config.default_file_name = match (field, path.file_name()) {
            (Browse::Record, Some(name)) => name.to_string_lossy().into_owned(),
            (Browse::Record, None) => "battle.rec".to_string(),
            (Browse::Save, _) => saving.map_or_else(String::new, |s| s.name.clone()),
            _ => String::new(),
        };
        config.title = Some(
            match field {
                Browse::Record => "Record to",
                Browse::Replay => "Open a recording",
                Browse::Folder => "Add a robot folder",
                Browse::Save => saving.map_or("Save", |s| s.title),
                Browse::OpenBattle => "Open a battle file",
            }
            .to_string(),
        );
        match field {
            Browse::Record | Browse::Save => self.files.save_file(),
            Browse::Replay | Browse::OpenBattle => self.files.pick_file(),
            Browse::Folder => self.files.pick_directory(),
        }
        self.browsing = Some(field);
    }

    /// Shows the open file dialog, and puts what was picked where it was asked for: a
    /// recording opens, a robot folder is added.
    fn file_dialog(&mut self, ctx: &egui::Context) {
        let Some(field) = self.browsing else {
            return;
        };
        self.files.update(ctx);
        if let Some(path) = self.files.take_picked() {
            self.browsing = None;
            match field {
                Browse::Record => self.setup.record = path.display().to_string(),
                Browse::Replay => {
                    self.open_path = path.display().to_string();
                    self.open_replay(path);
                }
                Browse::Folder => {
                    let before = self.settings.clone();
                    if !self.settings.robot_folders.contains(&path) {
                        self.settings.robot_folders.push(path);
                        self.settings_changed(ctx, &before);
                    }
                }
                Browse::OpenBattle => self.open_battle(&path),
                Browse::Save => {
                    if let Some(saving) = self.saving.take()
                        && let Err(e) = std::fs::write(&path, saving.text_for(&path))
                    {
                        self.save_error = Some(format!("{}: {e}", path.display()));
                    }
                }
            }
        } else if *self.files.state() != DialogState::Open {
            self.browsing = None;
            self.saving = None;
        }
    }

    /// Opens a classic `.battle` file into the battle setup: its robots and rules.
    fn open_battle(&mut self, path: &std::path::Path) {
        let file = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| battle_file::parse(&text, self.setup.battle_rules()));
        match file {
            Ok(file) => {
                self.setup.open(file);
                self.picked_selected = None;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{}: {e}", path.display())),
        }
    }

    /// Asks where to save the battle setup as a classic `.battle` file, and saves it.
    fn save_battle(&mut self) {
        let file = battle_file::BattleFile {
            rules: self.setup.battle_rules(),
            robots: self.setup.robots.clone(),
        };
        self.save(screen::SaveRequest {
            title: "Save the battle",
            name: "battle.battle".to_string(),
            text: battle_file::write(&file),
            by_extension: Vec::new(),
        });
    }

    /// Asks where to save `request`'s text, and saves it there.
    fn save(&mut self, request: screen::SaveRequest) {
        self.saving = Some(request);
        self.browse(Browse::Save);
    }

    /// Why the last save failed, in a window until it is closed.
    fn save_error_window(&mut self, ctx: &egui::Context) {
        let Some(error) = &self.save_error else {
            return;
        };
        let mut close = false;
        egui::Window::new("Could not save")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(error.as_str());
                close = ui.button("OK").clicked();
            });
        if close {
            self.save_error = None;
        }
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
                        .hint_text("from --record, \"Record to\" or the website")
                        .desired_width((ui.available_width() - 90.0).clamp(120.0, 480.0)),
                );
                if browse_button(ui, "Choose a recording") {
                    app.browse(Browse::Replay);
                }
                if ui.button("Open").clicked() && !app.open_path.trim().is_empty() {
                    let path = PathBuf::from(app.open_path.trim());
                    app.open_replay(path);
                }
            });
            ui.label(
                RichText::new(
                    "A full recording replays exactly, with debug paint and output. A website \
                     replay (one round) shows what its viewer draws. Or drop a file here.",
                )
                .weak(),
            );
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
                            match app.folder_contents.get(i).cloned().flatten() {
                                Some(c) => ui.label(RichText::new(holds(&c)).weak()),
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
                        ui.label(
                            RichText::new("None yet: add a folder with robot jars or classes.")
                                .weak(),
                        );
                        ui.end_row();
                    }
                    ui.label("");
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut app.new_folder)
                                .hint_text("robot jars, or an IDE's output (e.g. bin)")
                                .desired_width((ui.available_width() - 90.0).clamp(120.0, 420.0)),
                        );
                        if browse_button(ui, "Choose a folder with robot jars or classes") {
                            app.browse(Browse::Folder);
                        }
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
                        ui.label(format!("{}, {robots}", sources(&app.env.jars)));
                        if app.listing.is_some() {
                            ui.spinner();
                        }
                        if ui
                            .button("Rescan")
                            .on_hover_text("Look for new robots in the folders")
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
                                "{} from ROBOCODES_ROBOT_JARS",
                                sources(&app.env_jars)
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
                    ui.label("Robots");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.robot_names, "Names");
                        ui.checkbox(&mut s.robot_energy, "Energy");
                    });
                    ui.end_row();
                    ui.label("Explosions");
                    ui.checkbox(&mut s.explosions, "Show hits and deaths exploding");
                    ui.end_row();
                    ui.label("Ground");
                    ui.checkbox(&mut s.ground, "Draw the floor tiles");
                    ui.end_row();
                    ui.label("Sentry border");
                    ui.checkbox(&mut s.sentry_border, "Show it when a sentry robot fights");
                    ui.end_row();
                    ui.label("Speed");
                    ui.checkbox(&mut s.rates, "Show the turns and frames per second");
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
                    rules_rows(ui, &mut s.rules);
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
        if s.rules != before.rules {
            setup.rules = s.rules;
        }
        if s.robot_folders != before.robot_folders {
            self.rescan(ctx);
        }
    }

    /// The Start Battle tab with a battle (see [`screen`]): the battlefield, the controls
    /// below it, the robots' output in tabs, and the robots on the right; at its end the
    /// results.
    fn live_ui(&mut self, root: &mut egui::Ui) -> Next {
        let ctx = &root.ctx().clone();
        let mut next = Next::Stay;
        let style = self.style();
        let Some(live) = &mut self.live else {
            return Next::Setup;
        };
        let latest = live.latest.as_ref();
        screen::robots_panel(
            root,
            "robots",
            &live.robots,
            latest,
            &live.history,
            live.history.len(),
            &mut self.selected,
            true,
        );
        let (output, properties) = (&live.output, &live.properties);
        let event = screen::output_panel(
            root,
            "output",
            &live.robots,
            latest,
            &mut self.selected,
            &live.paint,
            |i| Cow::Borrowed(output.get(i).map_or("", String::as_str)),
            |i| properties.get(i).map_or_else(Cow::default, Cow::Borrowed),
            live.outcome.is_none() && !live.stopping,
        );
        let mut save = None;
        match event {
            Some(screen::OutputEvent::TogglePaint(i)) => {
                let on = !live.paint[i];
                live.set_paint(i, on);
            }
            Some(screen::OutputEvent::Save(i, text)) => {
                save = Some(screen::output_save(&live.robots[i], text));
            }
            Some(screen::OutputEvent::Clear(i)) => {
                if let Some(out) = live.output.get_mut(i) {
                    out.clear();
                }
            }
            Some(screen::OutputEvent::Kill(i)) => live.kill(i),
            None => {}
        }
        screen::controls_panel(root, "controls", |ui| {
            ui.horizontal(|ui| {
                let running = live.outcome.is_none() && !live.stopping;
                if screen::control(ui, running, "Stop the battle", |p, c, color| {
                    icons::stop(p, c, 6.0, color)
                }) {
                    live.stop();
                }
                let paused = live.paused;
                let hint = if paused { "Resume" } else { "Pause" };
                if screen::control(ui, running, hint, |p, c, color| {
                    if paused {
                        icons::play(p, c, 7.0, color);
                    } else {
                        icons::pause(p, c, 6.0, color);
                    }
                }) {
                    live.set_paused(!paused);
                }
                if screen::control(ui, running && paused, "One turn", |p, c, color| {
                    icons::step(p, c, 6.5, 1.0, color)
                }) {
                    live.step();
                }
                screen::divider(ui);
                let mut tps = live.turns_per_second;
                let (ended, stopping) = (live.outcome.is_some(), live.stopping);
                let changed = screen::speed_and_readouts(
                    ui,
                    &mut tps,
                    Some(&live.seed.to_string()),
                    |ui| {
                        if ended {
                            let button = egui::Button::new(RichText::new("New battle").strong())
                                .min_size(egui::vec2(0.0, 26.0));
                            if ui.add(button).clicked() {
                                next = Next::Setup;
                            }
                            ui.add_space(8.0);
                        } else if stopping {
                            ui.label(RichText::new("Stopping…").weak());
                            ui.spinner();
                            ui.add_space(8.0);
                        }
                    },
                    |ui| {
                        if let Some(s) = &live.latest {
                            screen::info(ui, "Turn", s.turn.to_string());
                            ui.add_space(10.0);
                            screen::info(ui, "Round", format!("{} / {}", s.round + 1, live.rounds));
                        }
                    },
                    |_| {},
                );
                if changed {
                    live.set_speed(tps);
                }
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(root, |ui| {
                let field = Field {
                    width: live.width,
                    height: live.height,
                    robots: &live.robots,
                    sentry_border: live.sentry_border,
                };
                let available = ui.max_rect();
                let size = (live.width, live.height);
                let navigation = view::Navigation::Modified;
                view::navigate(ui, &mut live.camera, size, available, navigation);
                let (t, rect) = view::draw(
                    ui,
                    &mut self.sprites,
                    &field,
                    style,
                    (live.previous.as_ref(), live.latest.as_ref()),
                    available,
                    &live.camera,
                );
                if live.outcome.is_none() && !live.stopping {
                    let layer = ui.layer_id();
                    for event in live.interaction.events(ui.ctx(), layer, rect, &t) {
                        live.input(event);
                    }
                }
            });
        match &live.outcome {
            Some(Ok(outcome)) => {
                let title = results_title(outcome);
                let shown = screen::Outcome {
                    title: &title,
                    robots: &live.robots,
                    results: &outcome.results,
                    snapshot: live.latest.as_ref(),
                    stopped: outcome.aborted,
                };
                let id = egui::Id::new(("results", self.battles));
                let sprites = &mut self.sprites;
                let request = screen::results_window(ctx, sprites, id, &shown, |ui| {
                    let replays = if self.setup.classic {
                        "Classic mode: the robots' timing differs, so it may go differently."
                    } else {
                        "Deterministic mode: the battle repeats exactly."
                    };
                    let restart = egui::Button::new(RichText::new("Restart Battle").strong());
                    if ui
                        .add(restart)
                        .on_hover_text(format!(
                            "The same robots and settings, with seed {}. {replays}",
                            live.seed
                        ))
                        .clicked()
                    {
                        next = Next::Restart { new_seed: false };
                    }
                    if ui
                        .button("Restart, new seed")
                        .on_hover_text("The same robots and settings with another seed.")
                        .clicked()
                    {
                        next = Next::Restart { new_seed: true };
                    }
                    if ui
                        .button("New Battle")
                        .on_hover_text("Back to the setup, with these robots.")
                        .clicked()
                    {
                        next = Next::Setup;
                    }
                });
                if request.is_some() {
                    save = request;
                }
            }
            Some(Err(e)) => {
                egui::Window::new("Battle failed").show(ctx, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, e);
                    if ui.button("Back").clicked() {
                        next = Next::Setup;
                    }
                });
            }
            None => {}
        }
        if let Some(save) = save {
            self.save(save);
        }
        next
    }
}

/// The width of the label column on the options page.
const LABELS: f32 = 150.0;

/// A section heading on a page: a title over a line, with space above unless it comes
/// first.
/// The folder button after a file or folder field; true when clicked.
fn browse_button(ui: &mut egui::Ui, hint: &str) -> bool {
    icons::button(ui, 22.0, hint, false, |p, c, color| {
        icons::folder(p, c, 7.0, color);
    })
    .clicked()
}

/// The rules' rows in a two-column grid: gun cooling rate, inactivity time, sentry border
/// and hiding enemy names.
fn rules_rows(ui: &mut egui::Ui, rules: &mut Rules) {
    ui.label("Gun cooling rate");
    ui.add(
        egui::DragValue::new(&mut rules.gun_cooling_rate)
            .range(0.01..=1.0)
            .speed(0.01)
            .fixed_decimals(2),
    )
    .on_hover_text("Gun heat lost per turn (classic's default 0.1)");
    ui.end_row();
    ui.label("Inactivity time");
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut rules.inactivity_time).range(0..=1_000_000));
        ui.label("turns");
    })
    .response
    .on_hover_text("Turns without damage before the robots start losing energy");
    ui.end_row();
    ui.label("Sentry border");
    ui.add(egui::DragValue::new(&mut rules.sentry_border).range(50..=1000))
        .on_hover_text("How far into the battlefield a sentry robot's border reaches");
    ui.end_row();
    ui.label("Enemy names");
    ui.checkbox(&mut rules.hide_enemy_names, "Hide them from the robots");
    ui.end_row();
}

fn section(ui: &mut egui::Ui, title: &str) {
    if ui.cursor().top() > ui.max_rect().top() {
        ui.add_space(20.0);
    }
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

/// A framed, scrolling list of robots, as wide as the column (so its scrollbar lines up
/// with the field above it).
fn list_box(ui: &mut egui::Ui, id: &str, height: f32, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(shell::BAR)
        .stroke(egui::Stroke::new(1.0, shell::LINE))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt(id)
                .auto_shrink([false, false])
                .min_scrolled_height(height)
                .max_height(height)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    content(ui);
                });
        });
}

/// What a robot row's icon does.
#[derive(Clone, Copy)]
enum Action {
    Add,
    Remove,
}

/// What happened to a robot row: clicked (it is highlighted), or its action asked for (a
/// double-click, or a click on its icon).
struct Row {
    clicked: bool,
    act: bool,
}

/// A row of a robot list, as wide as the list: the name, "team" for a team, and the
/// action's icon when the row is hovered or highlighted.
fn robot_row(ui: &mut egui::Ui, name: &str, team: bool, picked: bool, action: Action) -> Row {
    const HEIGHT: f32 = 24.0;
    // Room on the right for the scrollbar, which floats over the rows.
    const RIGHT: f32 = 10.0;
    let size = egui::vec2(ui.available_width(), HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let hint = match action {
        Action::Add => "Double-click to add (again for another one)",
        Action::Remove => "Double-click to remove",
    };
    let response = response.on_hover_text_at_pointer(hint);
    let visuals = ui.visuals();
    let hovered = response.hovered();
    let painter = ui.painter_at(rect);
    if picked || hovered {
        let fill = if picked {
            visuals.selection.bg_fill.gamma_multiply(0.6)
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(rect, 3.0, fill);
    }
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.right() - RIGHT - 10.0, rect.center().y),
        egui::vec2(20.0, 20.0),
    );
    let on_icon = response
        .interact_pointer_pos()
        .or(response.hover_pos())
        .is_some_and(|p| icon.contains(p));
    let mut right = icon.left() - 4.0;
    if team {
        let tag = painter.text(
            egui::pos2(right, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "team",
            egui::FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
        right = tag.left() - 6.0;
    }
    let galley = painter.layout_no_wrap(
        name.to_string(),
        egui::FontId::proportional(14.0),
        visuals.text_color(),
    );
    let at = egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0);
    let clip = egui::Rect::from_min_max(rect.min, egui::pos2(right, rect.max.y));
    painter
        .with_clip_rect(clip)
        .galley(at, galley, visuals.text_color());
    if picked || hovered {
        let color = if on_icon {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        if on_icon {
            painter.rect_filled(icon, 3.0, visuals.widgets.active.weak_bg_fill);
        }
        match action {
            Action::Add => icons::plus(&painter, icon.center(), 5.0, color),
            Action::Remove => icons::cross(&painter, icon.center(), 4.0, color),
        }
    }
    let clicked = response.clicked();
    Row {
        clicked,
        act: response.double_clicked() || (clicked && on_icon),
    }
}

/// The contestants on the loading screen: their tanks with their names, the radars turning
/// while they start. The tanks are in the colours a robot gets when it sets none; its own
/// are only known once the battle runs.
fn contestants_row(ui: &mut egui::Ui, sprites: &mut Sprites, names: &[String], time: f32) {
    const CELL: egui::Vec2 = egui::vec2(132.0, 92.0);
    let per_row = ((ui.available_width() / CELL.x).floor() as usize).max(1);
    let rows = names.len().div_ceil(per_row).max(1);
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), rows as f32 * CELL.y),
        egui::Sense::hover(),
    );
    let ctx = ui.ctx().clone();
    for (row, chunk) in names.chunks(per_row).enumerate() {
        let left = area.center().x - chunk.len() as f32 * CELL.x / 2.0;
        for (k, name) in chunk.iter().enumerate() {
            let i = row * per_row + k;
            let cell = egui::Rect::from_min_size(
                egui::pos2(left + k as f32 * CELL.x, area.top() + row as f32 * CELL.y),
                CELL,
            );
            let center = egui::pos2(cell.center().x, cell.top() + 32.0);
            // Default colours come from the class name, without version or duplicate suffix.
            let class = name.split(' ').next().unwrap_or(name);
            let colors = crate::sprites::default_colors(class);
            let radar = time * 2.0 + i as f32 * 0.7;
            for (part, color, heading) in [
                (Part::Body, colors[0], 0.0),
                (Part::Gun, colors[1], 0.0),
                (Part::Radar, colors[2], radar),
            ] {
                let (texture, size) = sprites.tank(&ctx, part, color);
                let shape = view::sprite(texture, center, size * 1.25, heading, Color32::WHITE);
                ui.painter().add(shape);
            }
            let label = egui::Rect::from_min_size(
                egui::pos2(cell.left() + 4.0, cell.top() + 62.0),
                egui::vec2(CELL.x - 8.0, 20.0),
            );
            ui.put(
                label,
                egui::Label::new(RichText::new(name).strong()).truncate(),
            );
        }
    }
    ui.ctx().request_repaint();
}

/// The robot jars and class folders in `paths`, e.g. `3 jars and 1 class folder`.
fn sources(paths: &[PathBuf]) -> String {
    let folders = paths.iter().filter(|p| p.is_dir()).count();
    let jars = match paths.len() - folders {
        1 => "1 jar".to_string(),
        n => format!("{n} jars"),
    };
    match folders {
        0 => jars,
        1 => format!("{jars} and 1 class folder"),
        n => format!("{jars} and {n} class folders"),
    }
}

/// What a robot folder holds, for the options: `2 jars`, `class files`, ...
fn holds(contents: &Contents) -> String {
    let jars = match contents.jars.len() {
        0 => None,
        1 => Some("1 jar".to_string()),
        n => Some(format!("{n} jars")),
    };
    match (jars, contents.classes) {
        (Some(jars), true) => format!("{jars} and class files"),
        (Some(jars), false) => jars,
        (None, true) => "class files".to_string(),
        (None, false) => "no jars or class files".to_string(),
    }
}

/// The results window's title: a stopped battle says in which round.
fn results_title(outcome: &BattleOutcome) -> String {
    if outcome.aborted {
        match outcome.rounds {
            1 => "Results: stopped in the first round".to_string(),
            n => format!("Results: stopped in round {n}"),
        }
    } else {
        "Results".to_string()
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if self.own_frame {
            crate::window::release_after_grab(ctx, raw_input);
        }
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let tab = self.tab;
        shell::top_bar(root, self.own_frame, &mut self.sprites, &mut self.tab);
        if self.tab == Tab::Start && tab != Tab::Start {
            self.refresh(ctx);
        }
        shell::footer(root);
        self.poll_listing();
        self.poll_starting(ctx);
        let style = self.style();
        // A battle keeps running in its tab while another one is open.
        if let Some(live) = &mut self.live {
            live.poll();
            if let (Some((turn, robot)), Some(s)) = (self.kill_at, &live.latest)
                && robot < live.robots.len()
                && (s.round, s.turn) >= (0, turn)
            {
                live.kill(robot);
                self.selected = robot;
                self.kill_at = None;
            }
            if let Some(s) = &live.latest
                && s.round == 0
            {
                let (due, later) = std::mem::take(&mut self.input_at)
                    .into_iter()
                    .partition(|(turn, _)| *turn <= s.turn);
                self.input_at = later;
                for event in due
                    .into_iter()
                    .flat_map(|(_, events): (i64, Vec<_>)| events)
                {
                    live.input(event);
                }
            }
        }
        match self.tab {
            Tab::Start if self.starting.is_some() => self.loading_page(root),
            Tab::Start if self.live.is_some() => match self.live_ui(root) {
                Next::Stay => {}
                Next::Setup => {
                    self.live = None;
                    self.refresh(ctx);
                }
                Next::Restart { new_seed } => {
                    let seed = match &self.live {
                        Some(live) if !new_seed => live.seed,
                        _ => random_seed(),
                    };
                    self.launch(ctx, seed);
                }
            },
            Tab::Start => self.start_page(root),
            Tab::Replay => match &mut self.replay {
                Some(replay) => {
                    let mut save = None;
                    let sprites = &mut self.sprites;
                    if replay.ui(root, sprites, style, &mut self.selected, &mut save) {
                        self.replay = None;
                    }
                    if let Some(save) = save {
                        self.save(save);
                    }
                }
                None => self.replay_page(root),
            },
            Tab::Rumble => self.rumble_page(root),
            Tab::Options => self.options_page(root),
        }
        self.file_dialog(ctx);
        self.save_error_window(ctx);
        // A file dropped on the window opens as a replay.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_path = path.display().to_string();
            self.open_replay(path);
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
        let ready = match (&self.starting, &self.live, self.tab) {
            // A battle that couldn't start: its error, once laid out.
            (Some(starting), _, Tab::Start) if starting.error.is_some() => shot.frames >= 3,
            (Some(_), _, Tab::Start) => shot.loading && shot.frames >= shot.after,
            // An ended battle: its results, once laid out.
            (None, Some(live), Tab::Start) if live.outcome.is_some() => {
                shot.ended += 1;
                shot.ended >= 30
            }
            (None, Some(live), Tab::Start) => live
                .latest
                .as_ref()
                .is_some_and(|s| s.turn >= shot.after || s.round > 0),
            _ => shot.frames >= shot.after,
        };
        if ready {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::{GAME_TYPES, Setup, battle_file, parse_robots};
    use crate::settings::Settings;

    #[test]
    fn a_game_type_sets_its_rules_and_is_recognised() {
        let mut setup = Setup::new(&Settings::default());
        let matching = |s: &Setup| GAME_TYPES.iter().filter(|g| g.matches(s)).count();
        assert!(GAME_TYPES[0].matches(&setup), "the defaults are classic's");
        setup.rules.inactivity_time = 900;
        assert_eq!(matching(&setup), 0, "custom");
        GAME_TYPES[2].apply(&mut setup);
        assert!(GAME_TYPES[2].matches(&setup));
        assert_eq!(matching(&setup), 1);
        assert_eq!((setup.width, setup.height, setup.rounds), (1000, 1000, 35));
    }

    #[test]
    fn a_battle_file_round_trips_through_the_setup() {
        let mut setup = Setup::new(&Settings::default());
        setup.robots = vec!["sample.Fire".into(), "sample.Fire".into()];
        setup.rules.hide_enemy_names = true;
        setup.rounds = 7;
        let file = battle_file::BattleFile {
            rules: setup.battle_rules(),
            robots: setup.robots.clone(),
        };
        let text = battle_file::write(&file);
        let mut opened = Setup::new(&Settings::default());
        opened.open(battle_file::parse(&text, opened.battle_rules()).unwrap());
        assert_eq!(opened.robots, setup.robots);
        assert_eq!(opened.battle_rules(), setup.battle_rules());
    }

    #[test]
    fn robots_from_the_command_line() {
        assert_eq!(
            parse_robots(" sample.Fire,sample.Walls , ,sample.Fire"),
            ["sample.Fire", "sample.Walls", "sample.Fire"]
        );
        assert!(parse_robots(" , ").is_empty());
    }
}
