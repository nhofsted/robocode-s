// SPDX-License-Identifier: EPL-2.0

//! The application: the tabs (battle setup and a live battle, watched battles and
//! replays, options) in the shell (see [`crate::shell`]).

use crate::backdrop::Backdrop;
use crate::icons;
use crate::ide::{self, Ide};
use crate::live::Live;
use crate::replay::{Opening, ReplayView, Request};
use crate::rerun;
use crate::rumble::Rumble;
use crate::screen;
use crate::settings::{Contents, RobotColors, Rules, Settings};
use crate::shell::{self, Tab};
use crate::sound::{self, Sound};
use crate::sprites::{Part, Sprites};
use crate::view::{self, Field, Style};
use eframe::egui::{self, Color32, RichText};
use egui_file_dialog::{DialogState, FileDialog, OpeningMode};
use robocode_s_engine::BattleRules;
use robocode_s_runner::ide::{marker, parse_marker};
use robocode_s_runner::local::{
    Environment, Listed, LocalBattle, list_robots, prepare_ide, random_seed,
};
use robocode_s_runner::pool::Pool;
use robocode_s_runner::{JavaHost, Sandbox, docker_pulling};
use robocode_s_server::tls::Fingerprint;
use robocode_s_server::{BattleOutcome, ServerConfig, battle_file};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

mod options;
mod start;
mod watch;

use options::{browse_button, expand_home, game_rows, rules_and_output_rows, sources};

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
    /// Start paused, at the first turn.
    pub paused: bool,
    pub turns_per_second: u32,
    pub rules: Rules,
    /// A battle to watch (Watch Battle, Remote Server): the server's address (`host:port`),
    /// observer token and certificate fingerprint (`sha256:...`, empty for none).
    pub watch: String,
    pub watch_token: String,
    pub watch_fingerprint: String,
}

impl Setup {
    /// A new battle with the defaults from the options.
    pub fn new(settings: &Settings) -> Self {
        Self {
            robots: settings.robots.clone(),
            rounds: settings.rounds,
            seed: String::new(),
            width: settings.width,
            height: settings.height,
            classic: settings.classic,
            record: String::new(),
            paint: false,
            paused: settings.start_paused,
            turns_per_second: 30,
            rules: settings.rules,
            watch: settings.watch.clone(),
            watch_token: String::new(),
            watch_fingerprint: settings.watch_fingerprint.clone(),
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

/// The most robot JVMs kept started between battles.
const MAX_WARM_HOSTS: usize = 2;

/// `--battle`: robots and teams separated by commas.
pub fn parse_robots(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(String::from)
        .collect()
}

/// The robots of the battle as the options keep them: without those from an IDE.
fn saved_robots(robots: &[String]) -> Vec<String> {
    robots
        .iter()
        .filter(|r| parse_marker(r).is_none())
        .cloned()
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
    /// It starts paused.
    paused: bool,
    /// Why the battle couldn't start.
    error: Option<String>,
    /// A battle on another server, watched: its address. The contestants and settings
    /// come once it is connected.
    watching: Option<String>,
    /// What its robots run in ([`Sandbox::report`]); `None` when watched.
    sandbox: Option<String>,
    /// A watched battle's pinned certificate.
    pin: Option<String>,
}

/// A battle with what comes before it: the loading screen while it starts, then the
/// battle screen.
#[derive(Default)]
struct Battle {
    live: Option<Box<Live>>,
    starting: Option<Starting>,
}

impl Battle {
    /// Moves a starting battle along: once it is described the battle starts, and with its
    /// first snapshot (or its end) the loading screen gives way to it. True when the
    /// battle screen is new.
    fn poll_starting(&mut self, ctx: &egui::Context, camera: view::Camera) -> bool {
        let Some(starting) = &mut self.starting else {
            return false;
        };
        let mut started = false;
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
                    live.camera = camera;
                    live.seed = starting.seed;
                    live.set_speed(starting.turns_per_second);
                    // Before its first turn is fought: the server pauses after it, so the
                    // battle screen opens on the battlefield as it starts.
                    if starting.paused {
                        live.set_paused(true);
                    }
                    if starting.paint {
                        for i in 0..live.robots.len() {
                            live.set_paint(i, true);
                        }
                    }
                    self.live = Some(Box::new(live));
                    started = true;
                }
                Err(e) => starting.error = Some(e),
            }
        }
        if let Some(live) = &self.live {
            // A watched battle's contestants and settings, once it is connected.
            if starting.watching.is_some()
                && starting.contestants.is_empty()
                && !live.robots.is_empty()
            {
                starting.contestants = live.robots.iter().map(|r| r.name.clone()).collect();
                starting.seed = live.seed;
                starting.rounds = live.rounds;
                starting.size = (live.width as i32, live.height as i32);
                starting.classic = !live.deterministic;
            }
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
        started
    }

    /// Closes the battle: one that runs here stops (see `Live`'s drop), a watched one
    /// goes on without us; robots still being described are left to finish, and dropped.
    fn close(&mut self) {
        self.starting = None;
        self.live = None;
    }
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
    /// `ROBOCODES_PERF_LOG` (see [`crate::perf`]).
    perf: Option<crate::perf::Perf>,
    sprites: Sprites,
    backdrop: Backdrop,
    env: Environment,
    /// Robot JVMs kept started between battles (the option "Warm robots"); dropped, they
    /// stop.
    pool: Option<Arc<Pool>>,
    setup: Setup,
    tab: Tab,
    /// The battle run here, in the Start Battle tab; the setup when there is none.
    local: Battle,
    /// The setup of the last battle started, for Restart.
    launched: Option<Setup>,
    /// A battle on a server, watched in the Watch Battle tab. It and a replay take turns
    /// there: the one opened last replaces the other.
    remote: Battle,
    /// The recording in the Watch Battle tab; with no battle watched either, the page to
    /// open one.
    replay: Option<Box<ReplayView>>,
    /// A recording being opened: the Watch Battle tab shows its progress.
    opening: Option<Opening>,
    selected: usize,
    error: Option<String>,
    /// Why the last recording couldn't be opened, or the last battle not watched.
    replay_error: Option<String>,
    remote_error: Option<String>,
    screenshot: Option<Screenshot>,
    /// `--kill-at`, until done.
    kill_at: Option<(i64, usize)>,
    /// `--full`, until the replay is open.
    full_on_open: bool,
    /// The full recording [`Browse::SaveRecording`] saves: the file, and the name to suggest.
    recording_to_save: Option<(PathBuf, String)>,
    /// Why the last full recording couldn't be made, in a window until it is closed.
    full_recording_error: Option<String>,
    /// `--input-at`, by turn, until sent.
    input_at: Vec<(i64, Vec<robocode_s_protocol::v1::InputEvent>)>,
    /// How battles and replays first show the battlefield (`--zoom`; fitted by default).
    camera: view::Camera,
    /// The sound effects' output, opened when sound is first on.
    sound: Sound,
    open_path: String,
    /// Robots started by the user's IDE, waiting for a battle.
    ide: Ide,
    /// `--ide-add`: add robots from an IDE to the battle as they connect.
    ide_add: bool,
    /// The robots in the jars and class folders, listed in the background.
    available: Option<Result<Vec<Listed>, String>>,
    listing: Option<Receiver<Result<Vec<Listed>, String>>>,
    /// List the robots again once the running listing is done.
    relist: bool,
    /// A newer release: the check, the notice and the update.
    updater: crate::update::Updater,
    /// The robot whose details show below the robot lists: the last one clicked.
    details: Option<String>,
    filter: String,
    /// The highlighted robot in the available list (by name) and in the battle (by index).
    picked: Option<String>,
    picked_selected: Option<usize>,
    /// Draw the window frame ourselves (see [`crate::window`]).
    own_frame: bool,
    /// The smallest the window gets, in points.
    min_size: egui::Vec2,
    /// The options, saved on every change.
    settings: Settings,
    /// Robot jars and class folders from `ROBOCODES_ROBOT_JARS`, besides the robot folders.
    env_jars: Vec<PathBuf>,
    /// What each robot folder holds; `None` for a folder that isn't there.
    folder_contents: Vec<Option<Contents>>,
    new_folder: String,
    settings_error: Option<String>,
    /// The options changed and aren't saved yet: since when (the UI's time). They are
    /// saved once they have been still for a moment, not on every frame of a drag.
    settings_unsaved: Option<f64>,
    /// The robots in the battle when [`App::save_settings`] last looked.
    robots_seen: Vec<String>,
    /// The battles started, so each one's results window opens centred.
    battles: u64,
    /// The file dialog behind the fields' folder buttons (drawn by egui: WSLg has no
    /// native one), and the field it is open for. It opens where the last pick was.
    files: FileDialog,
    browsing: Option<Browse>,
    /// The text [`Browse::Save`] saves, and why the last save failed.
    saving: Option<screen::SaveRequest>,
    save_error: Option<String>,
    /// The Run Rumble page: the sign-in and the contributing thread, which go on while
    /// other pages are open.
    rumble: Rumble,
}

/// The field a file dialog is open for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Browse {
    /// Start Battle's "Record to": a file to save.
    Record,
    /// Watch Battle's recording: a file to open.
    Replay,
    /// A robot folder for the options.
    Folder,
    /// A file to save [`App::saving`] to.
    Save,
    /// A classic `.battle` file to open into the battle setup.
    OpenBattle,
    /// A file to save [`App::recording_to_save`] to.
    SaveRecording,
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
    /// Watch the battle at the setup's address (`--watch`).
    pub watch: bool,
    pub replay: Option<PathBuf>,
    /// Turn the replay into a full recording once it is open, as its "Full recording"
    /// button does (`--full`).
    pub full: bool,
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
    /// The smallest the window gets, in points (see [`crate::window::keep_min_size`]).
    pub min_size: egui::Vec2,
    /// Open the "From your IDE…" panel at start, and add the robots that connect to the
    /// battle (`--from-ide`, `--ide-add`, for screenshots and testing).
    pub ide_panel: bool,
    pub ide_add: bool,
    /// Show the Run Rumble page in a made-up state (`--rumble-demo`, for screenshots).
    pub rumble_demo: Option<String>,
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
        // Dark whatever the system's theme: egui would follow a light Windows or macOS
        // theme (white text fields, and the title on the dark top bar near invisible),
        // and the app's colours (the top bar, the floor) are made for the dark one.
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        crate::fonts::install(&cc.egui_ctx);
        if own_frame {
            crate::window::hide_system_border(cc, shell::BAR);
        } else {
            // The system's title bar too, where it has a dark one (Windows, GTK).
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
        }
        let env_jars = crate::settings::env_jars();
        env.jars = settings.robot_paths(&env_jars);
        env.sandbox = settings.sandbox();
        let perf = crate::perf::Perf::from_env();
        if perf.is_some() {
            crate::perf::Perf::renderer(cc);
        }
        let robots_seen = launch.setup.robots.clone();
        let mut app = Self {
            perf,
            sprites: Sprites::new(&cc.egui_ctx),
            backdrop: Backdrop::default(),
            env,
            pool: None,
            setup: launch.setup,
            tab: Tab::Start,
            local: Battle::default(),
            launched: None,
            remote: Battle::default(),
            replay: None,
            selected: 0,
            error: None,
            replay_error: None,
            remote_error: None,
            opening: None,
            screenshot,
            kill_at: launch.kill_at,
            full_on_open: launch.full,
            recording_to_save: None,
            full_recording_error: None,
            input_at: launch.input_at,
            camera: launch.camera,
            sound: Sound::default(),
            open_path: String::new(),
            ide: Ide::new(launch.ide_panel),
            ide_add: launch.ide_add,
            available: None,
            listing: None,
            relist: false,
            details: None,
            filter: String::new(),
            picked: None,
            picked_selected: None,
            own_frame,
            min_size: launch.min_size,
            folder_contents: Vec::new(),
            settings,
            env_jars,
            new_folder: String::new(),
            settings_error: None,
            settings_unsaved: None,
            robots_seen,
            battles: 0,
            files: FileDialog::with_file_system(std::sync::Arc::new(crate::files::Files))
                .id("file-dialog")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]),
            browsing: None,
            saving: None,
            save_error: None,
            updater: crate::update::Updater::new(),
            rumble: launch
                .rumble_demo
                .as_deref()
                .and_then(Rumble::demo)
                .unwrap_or_default(),
        };
        crate::graphs::set_whole_battle(&cc.egui_ctx, app.settings.graphs_battle);
        robocode_s_update::cleanup();
        // Not for the screenshots of tests and docs, unless they show a test release.
        let test_release = std::env::var_os(robocode_s_update::URL_VAR).is_some();
        if app.settings.check_updates && (app.screenshot.is_none() || test_release) {
            app.updater.start_daily(&cc.egui_ctx);
        }
        app.scan_folders();
        app.list_robots(&cc.egui_ctx);
        // A battle here and one watched (or a replay) have tabs of their own: both start.
        if launch.start {
            app.start(&cc.egui_ctx);
        }
        if let Some(path) = launch.replay {
            app.open_replay(&cc.egui_ctx, path);
        } else if launch.watch {
            app.watch(&cc.egui_ctx);
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

    /// Lists the robots in the jars on a background thread (it starts a JVM), each listing
    /// in a home of its own below `data/list`. One listing at a time: asked again while
    /// one runs, it lists again once that one is done.
    fn list_robots(&mut self, ctx: &egui::Context) {
        if self.listing.is_some() {
            self.relist = true;
            return;
        }
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

    /// Takes the robot list once it is there (or lists again, if the folders changed
    /// meanwhile).
    fn poll_listing(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.listing
            && let Ok(result) = rx.try_recv()
        {
            self.listing = None;
            if std::mem::take(&mut self.relist) {
                self.list_robots(ctx);
            } else {
                self.available = Some(result);
            }
        }
    }

    /// Starts the battle in the setup: the loading screen while the robots are described
    /// (on a background thread) and their JVMs start.
    fn start(&mut self, ctx: &egui::Context) {
        if self.setup.robots.is_empty() {
            self.error = Some("Add some robots to the battle first.".into());
            return;
        }
        if let Some(name) = self.ide.missing(&self.setup.robots) {
            self.error = Some(format!(
                "{} isn't connected: start it again from your IDE, or take it out of the battle.",
                ide::label(&name)
            ));
            return;
        }
        let seed = match parse_seed(&self.setup.seed) {
            Ok(seed) => seed.unwrap_or_else(random_seed),
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.launch(ctx, self.setup.clone(), seed);
    }

    /// Starts the battle of setup `s` with `seed` (see [`App::start`]).
    fn launch(&mut self, ctx: &egui::Context, s: Setup, seed: i64) {
        let config = ServerConfig {
            rules: s.battle_rules(),
            seed,
            deterministic: !s.classic,
            record: (!s.record.trim().is_empty()).then(|| PathBuf::from(s.record.trim())),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        let (env, robots, repaint) = (self.env.clone(), s.robots.clone(), ctx.clone());
        let ide = self.ide.listener();
        // The robots' JVMs start while they are described, and the next battle's while
        // this one runs.
        if !self.settings.warm_hosts {
            self.pool = None;
        } else if self.pool.is_none() {
            let dir = env.data.join("pool").join(std::process::id().to_string());
            self.pool = Some(Arc::new(Pool::new(dir)));
        }
        let pool = self.pool.clone();
        let from_ide = robots.iter().filter(|r| parse_marker(r).is_some()).count();
        let warm = (robots.len() - from_ide).min(MAX_WARM_HOSTS);
        std::thread::spawn(move || {
            let warm = pool.as_ref().map(|pool| (pool, warm));
            // Cancelled meanwhile: nobody fights the battle, which leaves its folder.
            if let Err(mpsc::SendError(Ok((_, battle)))) =
                tx.send(prepare_ide(&env, &robots, ide.as_ref(), config, 512, warm))
            {
                battle.remove_dir();
            }
            repaint.request_repaint();
        });
        self.local.starting = Some(Starting {
            contestants: s.robots.iter().map(|r| self.ide.display(r)).collect(),
            prepared: Some(rx),
            seed,
            rounds: s.rounds.max(1),
            size: (s.width, s.height),
            classic: s.classic,
            turns_per_second: s.turns_per_second,
            paint: s.paint,
            paused: s.paused,
            error: None,
            watching: None,
            sandbox: Some(self.env.sandbox.report()),
            pin: None,
        });
        self.launched = Some(s);
        self.error = None;
        self.local.live = None;
        self.battles += 1;
        self.tab = Tab::Start;
    }

    /// Watches the battle at the setup's address as an observer, in the Watch Battle tab:
    /// the loading screen until it is connected and its first turn is there. It replaces
    /// a replay there, as a replay opened later replaces it.
    fn watch(&mut self, ctx: &egui::Context) {
        let address = self.setup.watch.trim().to_string();
        self.tab = Tab::Watch;
        if address.is_empty() {
            self.remote_error = Some("Give the address of the battle to watch.".into());
            return;
        }
        let fingerprint = self.setup.watch_fingerprint.trim().to_string();
        let pin = if fingerprint.is_empty() {
            None
        } else {
            match fingerprint.parse::<Fingerprint>() {
                Ok(pin) => Some(pin),
                Err(e) => {
                    self.remote_error = Some(e);
                    return;
                }
            }
        };
        if self.settings.watch != address || self.settings.watch_fingerprint != fingerprint {
            self.settings.watch = address.clone();
            self.settings.watch_fingerprint = fingerprint;
            self.settings_unsaved = Some(ctx.input(|i| i.time));
        }
        let token = self.setup.watch_token.clone();
        let mut live = Live::watch(ctx, address.clone(), token, pin);
        live.camera = self.camera;
        live.set_speed(self.setup.turns_per_second);
        self.remote.starting = Some(Starting {
            contestants: Vec::new(),
            prepared: None,
            seed: 0,
            rounds: 1,
            size: (800, 600),
            classic: false,
            turns_per_second: self.setup.turns_per_second,
            paint: false,
            paused: false,
            error: None,
            watching: Some(address),
            sandbox: None,
            pin: pin.map(|p| p.to_string()),
        });
        self.selected = 0;
        self.remote.live = Some(Box::new(live));
        self.remote_error = None;
        self.replay = None;
        self.opening = None;
        self.replay_error = None;
        self.battles += 1;
    }

    /// Moves the starting battles along (see [`Battle::poll_starting`]).
    fn poll_starting(&mut self, ctx: &egui::Context) {
        for battle in [&mut self.local, &mut self.remote] {
            if battle.poll_starting(ctx, self.camera) {
                self.selected = 0;
            }
        }
    }

    /// Opens a recording in the Watch Battle tab, on a thread of its own (it is replayed
    /// once, which takes a while for a long battle); one opening before cancels. Once it
    /// is open it replaces the replay or watched battle before; until then Cancel goes
    /// back to that.
    fn open_replay(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.tab = Tab::Watch;
        self.replay_error = None;
        self.opening = Some(Opening::start(ctx, path));
    }

    /// Takes the recording being opened once it is there, or why it couldn't be.
    fn poll_opening(&mut self) {
        let Some(result) = self.opening.as_ref().and_then(Opening::poll) else {
            return;
        };
        let recreating = self.opening.as_ref().is_some_and(|o| o.status().is_some());
        let path = self
            .opening
            .take()
            .map(|o| o.path.clone())
            .unwrap_or_default();
        // A track that couldn't become a full recording stays.
        if recreating && let Err(e) = &result {
            self.full_recording_error = Some(e.clone());
            return;
        }
        // The Watch Battle page shows the new replay, or the error: not what was there.
        self.remote.close();
        self.remote_error = None;
        match result {
            Ok(mut view) => {
                self.selected = 0;
                view.camera = self.camera;
                self.replay = Some(Box::new(view));
            }
            Err(e) => {
                self.replay = None;
                self.replay_error = Some(format!("{}: {e}", path.display()));
            }
        }
    }

    /// Turns the track in the Watch Battle tab into a full recording, which then replaces
    /// it there (see [`rerun`]), if the track names its battle.
    fn full_recording(&mut self, ctx: &egui::Context) {
        let Some(replay) = &self.replay else {
            return;
        };
        let Some(battle) = replay.battle().cloned() else {
            return;
        };
        let track = replay.path().to_path_buf();
        let server = crate::rumble::server_now(&self.settings.rumble, None);
        let out = rerun::recording_path(&self.env.data, &track);
        self.opening = Some(Opening::recreate(
            ctx,
            track,
            battle,
            self.env.clone(),
            server,
            out,
        ));
        self.replay_error = None;
    }

    /// Why the last full recording couldn't be made, in a window until it is closed.
    fn full_recording_error_window(&mut self, ctx: &egui::Context) {
        let Some(error) = &self.full_recording_error else {
            return;
        };
        let mut close = false;
        egui::Window::new("No full recording")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(error.as_str());
                close = ui.button("OK").clicked();
            });
        if close {
            self.full_recording_error = None;
        }
    }

    /// A page over the backdrop (see [`shell::page`]).
    fn page(&mut self, root: &mut egui::Ui, content: impl FnOnce(&mut Self, &mut egui::Ui)) {
        shell::page(
            root,
            self,
            |app| (&mut app.backdrop, &mut app.sprites, app.settings.background),
            content,
        );
    }

    /// Opens the file dialog for `field`, starting from the field's folder if it has one
    /// (otherwise where the last pick was).
    pub fn browse(&mut self, field: Browse) {
        let text = match field {
            Browse::Record => &self.setup.record,
            Browse::Replay => &self.open_path,
            Browse::Folder => &self.new_folder,
            Browse::Save | Browse::OpenBattle | Browse::SaveRecording => "",
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
            (Browse::SaveRecording, _) => self
                .recording_to_save
                .as_ref()
                .map_or_else(String::new, |(_, name)| name.clone()),
            _ => String::new(),
        };
        config.title = Some(
            match field {
                Browse::Record => "Record to",
                Browse::Replay => "Open a recording",
                Browse::Folder => "Add a robot folder",
                Browse::Save => saving.map_or("Save", |s| s.title),
                Browse::OpenBattle => "Open a battle file",
                Browse::SaveRecording => "Save the recording",
            }
            .to_string(),
        );
        let filters: &[(&str, &'static [&'static str])] = match field {
            Browse::Replay => &[
                ("Recordings", &["rsr", "rec", "track"]),
                ("Website tracks", &["track"]),
            ],
            Browse::OpenBattle => &[("Battle files", &["battle"])],
            Browse::Record | Browse::Folder | Browse::Save | Browse::SaveRecording => &[],
        };
        crate::files::set_filters(config, filters);
        match field {
            Browse::Record | Browse::Save | Browse::SaveRecording => self.files.save_file(),
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
                    self.open_replay(ctx, path);
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
                Browse::SaveRecording => {
                    if let Some((from, _)) = self.recording_to_save.take()
                        && let Err(e) = std::fs::copy(&from, &path)
                    {
                        self.save_error = Some(format!("{}: {e}", path.display()));
                    }
                }
            }
        } else if *self.files.state() != DialogState::Open {
            self.browsing = None;
            self.saving = None;
            self.recording_to_save = None;
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

    /// The Run Rumble tab: signing in and contributing battles to the rumble.
    fn rumble_page(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        let mut changed = false;
        self.page(root, |app, ui| {
            changed = app.rumble.ui(ui, &mut app.settings, &app.env);
        });
        if changed {
            self.settings_unsaved = Some(ctx.input(|i| i.time));
        }
        if std::mem::take(&mut self.rumble.update_asked) {
            self.updater.look_now(ctx);
        }
    }

    /// The notice of a newer release, and installing it when the user agrees: the robot
    /// JVMs started from the installation stop first, and the app starts again after.
    fn update_notice(&mut self, ctx: &egui::Context) {
        let wait = if self.local.live.is_some() || self.local.starting.is_some() {
            Some("Close the battle first")
        } else if self.rumble.active() {
            Some("Stop running the rumble first")
        } else {
            None
        };
        match self.updater.ui(ctx, wait) {
            crate::update::Action::None => {}
            crate::update::Action::Install => {
                self.pool = None;
                self.updater.install(ctx);
            }
            crate::update::Action::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            crate::update::Action::Restart(exe) => match crate::update::restart(&exe) {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.updater.note(format!(
                    "The new release is installed, but it couldn't be started ({e}): start \
                     robocode-s again."
                )),
            },
        }
    }
}

/// The width of the label column on the options page.
const LABELS: f32 = 150.0;

/// A two-column grid of options: the labels, then their fields.
pub(crate) fn options_grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(LABELS)
        .spacing([12.0, 8.0])
        .show(ui, rows);
}

/// A section heading on a page: a title over a line, with space above unless it comes
/// first.
pub(crate) fn section(ui: &mut egui::Ui, title: &str) {
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

/// The frame of a [`list_box`]: dark, with a line, as wide as the column.
pub(crate) fn list_frame(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(shell::BAR)
        .stroke(egui::Stroke::new(1.0, shell::LINE))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
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

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if self.own_frame {
            crate::window::release_after_grab(ctx, raw_input);
        }
    }

    fn ui(&mut self, root: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        if let Some(perf) = &mut self.perf {
            perf.begin();
        }
        crate::pace::begin(ctx, self.settings.frame_rate);
        crate::window::keep_min_size(ctx, self.min_size);
        let tab = self.tab;
        shell::top_bar(root, self.own_frame, &mut self.sprites, &mut self.tab);
        if self.tab == Tab::Start && tab != Tab::Start {
            self.refresh(ctx);
        }
        shell::footer(root);
        self.poll_listing(ctx);
        self.poll_starting(ctx);
        self.poll_ide(ctx);
        // Contributing goes on, and a closing window waits for it, on any page.
        self.rumble.poll(ctx);
        self.rumble.closing(ctx);
        let style = self.style();
        // A battle keeps running in its tab while another tab is open.
        if let Some(live) = &mut self.remote.live {
            live.poll();
        }
        if let Some(live) = &mut self.local.live {
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
        self.remember_colors(ctx);
        self.poll_opening();
        if self.full_on_open && self.opening.is_none() {
            self.full_on_open = false;
            self.full_recording(ctx);
        }
        match self.tab {
            Tab::Start if self.local.starting.is_some() => self.loading_page(root, false),
            Tab::Start if self.local.live.is_some() => match self.live_ui(root, false) {
                Next::Stay => {}
                Next::Setup => {
                    self.local.live = None;
                    self.refresh(ctx);
                }
                Next::Restart { new_seed } => {
                    let seed = match &self.local.live {
                        Some(live) if !new_seed => live.seed,
                        _ => random_seed(),
                    };
                    // The battle as it was started (the options may have changed the
                    // setup's defaults since).
                    let mut setup = self.launched.clone().unwrap_or_else(|| self.setup.clone());
                    // The battle's recording stays: the new one goes to a new file.
                    let record = setup.record.trim();
                    if !record.is_empty() {
                        setup.record = next_free(Path::new(record)).display().to_string();
                        self.setup.record.clone_from(&setup.record);
                    }
                    self.launch(ctx, setup, seed);
                }
            },
            Tab::Start => self.start_page(root),
            Tab::Watch if self.opening.is_some() => self.opening_page(root),
            Tab::Watch if self.remote.starting.is_some() => self.loading_page(root, true),
            // A watched battle offers no restart: it goes back to the Watch Battle page.
            Tab::Watch if self.remote.live.is_some() => match self.live_ui(root, true) {
                Next::Stay => {}
                Next::Setup | Next::Restart { .. } => self.remote.close(),
            },
            Tab::Watch => match &mut self.replay {
                Some(replay) => {
                    let (mut save, mut request) = (None, None);
                    let sprites = &mut self.sprites;
                    if replay.ui(
                        root,
                        sprites,
                        style,
                        &mut self.selected,
                        &mut save,
                        &mut request,
                    ) {
                        self.replay = None;
                    }
                    if let Some(save) = save {
                        self.save(save);
                    }
                    match request {
                        Some(Request::FullRecording) => self.full_recording(ctx),
                        Some(Request::Save(path, name)) => {
                            self.recording_to_save = Some((path, name));
                            self.browse(Browse::SaveRecording);
                        }
                        None => {}
                    }
                }
                None => self.watch_page(root),
            },
            Tab::Rumble => self.rumble_page(root),
            Tab::Options => self.options_page(root),
        }
        self.ide_windows(ctx);
        self.update_notice(ctx);
        self.play_sounds(ctx);
        self.file_dialog(ctx);
        self.save_error_window(ctx);
        self.full_recording_error_window(ctx);
        // A file dropped on the window opens as a replay.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_path = path.display().to_string();
            self.open_replay(ctx, path);
        }
        self.save_settings(ctx);
        self.screenshot_step(ctx);
        if let Some(perf) = &mut self.perf {
            perf.end(ctx, frame);
        }
    }

    fn on_exit(&mut self) {
        // Stops the robot JVMs waiting for a battle.
        self.pool = None;
        if self.settings_unsaved.is_some() {
            let _ = self.settings.save();
        }
    }
}

impl App {
    /// Listens for robots from the IDE as the options say, and takes those that connected.
    fn poll_ide(&mut self, ctx: &egui::Context) {
        self.ide.sync(ctx, &self.env, &self.settings);
        if self.ide_add {
            let new: Vec<String> = self
                .ide
                .connected
                .iter()
                .map(|r| marker(r.id))
                .filter(|m| !self.setup.robots.contains(m))
                .collect();
            self.setup.robots.extend(new);
        }
    }

    /// The notice of a robot that connected and the "From your IDE…" panel.
    fn ide_windows(&mut self, ctx: &egui::Context) {
        let mut add = self.ide.notice_ui(ctx, 40.0);
        let (added, changed) =
            self.ide
                .panel(ctx, &self.env, &mut self.settings, &self.setup.robots);
        if changed {
            self.settings_unsaved = Some(ctx.input(|i| i.time));
        }
        add = add.or(added);
        // Already in the battle: nothing to announce.
        if let Some(n) = &self.ide.notice
            && self.setup.robots.contains(&marker(n.id))
        {
            self.ide.notice = None;
        }
        if let Some(id) = add {
            self.add_robot(marker(id));
            if self.local.live.is_none() && self.local.starting.is_none() {
                self.tab = Tab::Start;
            }
        }
    }

    /// Takes the tank colours of the robots in the battles into the options, for the
    /// loading screen of their next battle.
    fn remember_colors(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        for battle in [&mut self.local, &mut self.remote] {
            let Some(seen) = battle.live.as_mut().and_then(|live| live.take_colors()) else {
                continue;
            };
            let known = &mut self.settings.robot_colors;
            for (name, colors) in seen {
                changed |= match colors {
                    Some(colors) => known.set(&name, colors),
                    None => known.forget(&name),
                };
            }
        }
        if changed {
            self.settings_unsaved = Some(ctx.input(|i| i.time));
        }
    }

    /// Saves the options once they have been still for a moment and nothing is being
    /// dragged; takes the graphs' Round/Battle choice and the robots in the battle into
    /// them first.
    fn save_settings(&mut self, ctx: &egui::Context) {
        let battle = crate::graphs::whole_battle(ctx);
        if battle != self.settings.graphs_battle {
            self.settings.graphs_battle = battle;
            self.settings_unsaved = Some(ctx.input(|i| i.time));
        }
        // Only once the list changes, so that `--battle`'s robots don't replace the saved.
        if self.setup.robots != self.robots_seen {
            self.robots_seen = self.setup.robots.clone();
            let robots = saved_robots(&self.setup.robots);
            if robots != self.settings.robots {
                self.settings.robots = robots;
                self.settings_unsaved = Some(ctx.input(|i| i.time));
            }
        }
        let Some(since) = self.settings_unsaved else {
            return;
        };
        const STILL: f64 = 0.5;
        let (now, down) = ctx.input(|i| (i.time, i.pointer.any_down()));
        if now - since >= STILL && !down {
            self.settings_error = self.settings.save().err();
            self.settings_unsaved = None;
        } else {
            let wait = (STILL - (now - since)).max(0.05);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// Plays the sound effects of the battle on screen, when sound is on and it runs at a
    /// watchable speed; the rest are dropped.
    fn play_sounds(&mut self, ctx: &egui::Context) {
        let take = |battle: &mut Battle| {
            battle.live.as_mut().map(|live| {
                let tps = live.turns_per_second;
                (std::mem::take(&mut live.cues), tps)
            })
        };
        let (local, remote) = (take(&mut self.local), take(&mut self.remote));
        let replay = self
            .replay
            .as_mut()
            .map(|replay| std::mem::take(&mut replay.cues));
        if !self.settings.sound {
            return;
        }
        if self.sound.prepare() {
            // Look for the output thread's answer again soon.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let live = match self.tab {
            Tab::Start if self.local.starting.is_none() => local,
            Tab::Watch if self.opening.is_some() => return,
            Tab::Watch if self.remote.live.is_some() => match self.remote.starting {
                None => remote,
                Some(_) => return,
            },
            Tab::Watch => {
                self.sound
                    .play(&replay.unwrap_or_default(), self.settings.volume);
                return;
            }
            _ => return,
        };
        let cues = match live {
            Some((cues, tps)) if (1..=sound::MAX_TPS).contains(&tps) => cues,
            _ => return,
        };
        self.sound.play(&cues, self.settings.volume);
    }

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
        // The battle on screen, if any: a replay being opened shows over a watched one.
        let battle = match self.tab {
            Tab::Start => Some(&self.local),
            Tab::Watch if self.opening.is_none() => Some(&self.remote),
            _ => None,
        };
        let ready = match battle.map(|b| (&b.starting, &b.live)) {
            // A replay being opened: once it is there (or its loading screen).
            _ if self.tab == Tab::Watch && self.opening.is_some() => {
                shot.loading && shot.frames >= shot.after
            }
            // A battle that couldn't start: its error, once laid out.
            Some((Some(starting), _)) if starting.error.is_some() => shot.frames >= 3,
            Some((Some(_), _)) => shot.loading && shot.frames >= shot.after,
            // An ended battle: its results, once laid out.
            Some((None, Some(live))) if live.outcome.is_some() => {
                shot.ended += 1;
                shot.ended >= 30
            }
            // Started paused (`--paused`): it stays at its first turn, so frames count.
            Some((None, Some(live))) if live.paused => shot.frames >= shot.after,
            Some((None, Some(live))) => live
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

/// A recording's path for a restarted battle, so that it doesn't overwrite the last one:
/// `battle.rec` becomes `battle-2.rec` (`battle-3.rec`, ... when that one is there too).
fn next_free(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    // `battle-2` counts on from 2 rather than becoming `battle-2-2`.
    let base = match stem.rsplit_once('-') {
        Some((base, n)) if !base.is_empty() && n.parse::<u32>().is_ok() => base,
        _ => stem.as_str(),
    };
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2u32..)
        .map(|n| path.with_file_name(format!("{base}-{n}{extension}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

/// What Tank Royale shows of a bot besides its description: its programming language,
/// platform and game version (classic's `.properties` have no country).
fn robot_facts(robot: &Listed) -> String {
    [
        ("Language", &robot.language),
        ("Platform", &robot.platform),
        ("Robocode", &robot.robocode_version),
    ]
    .iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| format!("{k} {v}"))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// The seed field: empty for a random seed, or a Java `long` as the battle screen, the
/// results and `robocode-s` show it (negative too).
fn parse_seed(text: &str) -> Result<Option<i64>, String> {
    match text.trim() {
        "" => Ok(None),
        text => text.parse().map(Some).map_err(|_| {
            format!(
                "The seed is not a number from {} to {}: {text}",
                i64::MIN,
                i64::MAX
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GAME_TYPES, Listed, Setup, battle_file, next_free, parse_robots, parse_seed, robot_facts,
        saved_robots,
    };
    use robocode_s_runner::ide::marker;

    #[test]
    fn a_restart_records_to_a_new_file() {
        let dir = std::env::temp_dir().join(format!("robocode-s-gui-next-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("battle.rec");
        std::fs::write(&first, "").unwrap();
        assert_eq!(next_free(&first), dir.join("battle-2.rec"));
        std::fs::write(dir.join("battle-2.rec"), "").unwrap();
        assert_eq!(next_free(&first), dir.join("battle-3.rec"));
        assert_eq!(
            next_free(&dir.join("battle-2.rec")),
            dir.join("battle-3.rec")
        );
        assert_eq!(next_free(&dir.join("my-bots")), dir.join("my-bots-2"));
        assert_eq!(next_free(&dir.join("-7.rec")), dir.join("-7-2.rec"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn robot_facts_leave_out_what_is_unknown() {
        let robot = Listed {
            language: "Java".into(),
            platform: "JVM 21.0.4".into(),
            ..Default::default()
        };
        assert_eq!(robot_facts(&robot), "Language Java · Platform JVM 21.0.4");
        assert_eq!(robot_facts(&Listed::default()), "");
    }

    #[test]
    fn a_shown_seed_can_be_typed_back() {
        for seed in [0, 7, -8938542223750784803, i64::MIN, i64::MAX] {
            assert_eq!(parse_seed(&format!(" {seed} ")), Ok(Some(seed)));
        }
        assert_eq!(parse_seed("  "), Ok(None));
        assert!(parse_seed("9223372036854775808").is_err());
        assert!(parse_seed("12x").is_err());
    }
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

    #[test]
    fn the_battles_robots_are_kept_without_those_from_an_ide() {
        let settings = Settings {
            robots: vec!["sample.Fire".into(), "sample.Walls".into()],
            ..Settings::default()
        };
        assert_eq!(Setup::new(&settings).robots, settings.robots);
        let robots = ["sample.Fire".into(), marker(3), "sample.Fire".into()];
        assert_eq!(saved_robots(&robots), ["sample.Fire", "sample.Fire"]);
    }
}
