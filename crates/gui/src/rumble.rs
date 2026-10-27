// SPDX-License-Identifier: EPL-2.0

//! The Run Rumble page (docs/rumble-backend.md §13.7): signing in with GitHub's device flow,
//! and a [`Contributor`] running on a thread of its own, whose events the page shows. The
//! page's state is [`Model`] (what the events and the thread's messages make of it, with no
//! window needed) and [`Rumble`] (the threads, the account and the drawing).

use crate::app::{list_frame, options_grid, section};
use crate::settings::{RumbleSettings, Settings};
use crate::shell;
use eframe::egui::{self, Color32, RichText};
use robocode_s_engine::ENGINE_VERSION;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::Environment;
use rumble_client::backoff::{self, Backoff};
use rumble_client::battle::Options as BattleOptions;
use rumble_client::contributor::{
    Config, Contributor, Event, Fatal, MELEE_SLOTS, Stop, Summary, default_slots, offered_divisions,
};
use rumble_client::http::{self, Api};
use rumble_client::login::{DeviceCode, DeviceFlow, GITHUB};
use rumble_client::tokenfile::{self, Credentials, TokenFile};
use rumble_core::api::{BattleStatus, Me, Tier};
use rumble_core::ids::Division;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Where the rumble is, unless the options or `ROBOCODES_RUMBLE_SERVER` say otherwise.
pub const DEFAULT_SERVER: &str = "https://rumble.planethofstede.be";
const SERVER_VAR: &str = "ROBOCODES_RUMBLE_SERVER";
/// Where GitHub is (tests of the client point it at a fake).
const GITHUB_VAR: &str = "ROBOCODES_RUMBLE_GITHUB";

/// How long closing the window waits for the contributor to post what is done.
pub const CLOSE_WAIT: Duration = Duration::from_secs(10);

const RESULT_LINES: usize = 200;
const LOG_LINES: usize = 300;

/// `https://host` without the trailing slash, or nothing if `text` isn't a URL.
pub fn clean_server(text: &str) -> Option<String> {
    let url = text.trim().trim_end_matches('/');
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_string())
}

/// The server to talk to: `$ROBOCODES_RUMBLE_SERVER`, else the options' address, else the
/// one the token file was written for, else the public rumble. Text that isn't a URL is
/// passed over.
pub fn server_for(env: Option<&str>, option: &str, stored: Option<&str>) -> String {
    [env, Some(option), stored]
        .into_iter()
        .flatten()
        .find_map(clean_server)
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

pub fn server_now(settings: &RumbleSettings, stored: Option<&str>) -> String {
    let env = std::env::var(SERVER_VAR).ok();
    server_for(env.as_deref(), &settings.server, stored)
}

fn api_for(server: &str, token: Option<String>) -> Api {
    Api::new(&format!("{server}/api/v1"), token, ENGINE_VERSION)
}

// ---- the model ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// The start checks (§13.1) are running.
    Starting,
    Running,
    /// Stopped once: the running battles finish and are posted. A second stop abandons them.
    Stopping,
}

/// What the start checks found out.
#[derive(Clone, Debug, PartialEq)]
pub struct RunInfo {
    pub login: String,
    pub tier: Tier,
    pub slots: usize,
    pub divisions: Vec<Division>,
    pub platform: String,
    pub sandbox: String,
}

/// A battle on its way.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub batch: String,
    pub index: u32,
    pub division: Division,
    pub bots: Vec<String>,
    pub round: u32,
    pub of: u32,
}

/// What this session has done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub battles_ok: u32,
    pub robot_failed: u32,
    pub not_run: u32,
    pub errors: u32,
    pub batches_posted: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub uploaded: u32,
}

/// The account's numbers as the last post answered them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllTime {
    pub battles: u64,
    pub reputation: i64,
    pub tier: Tier,
}

impl AllTime {
    fn of(me: &Me) -> Self {
        Self {
            battles: me.battles,
            reputation: me.reputation,
            tier: me.tier,
        }
    }
}

/// Why contributing stopped, for the page to say.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// 401: sign in again.
    LoginNeeded,
    /// The engine or the client doesn't fit the rumble: update robocode-s.
    Update(String),
    /// 403, with the server's words.
    Refused(String),
    /// Anything else (the sandbox, the Java runtime, the server not answering, ...).
    Failed(String),
}

impl Problem {
    pub fn of(fatal: &Fatal) -> Problem {
        match fatal {
            Fatal::LoginNeeded => Problem::LoginNeeded,
            Fatal::Engine(m) | Fatal::ClientOutdated(m) => Problem::Update(m.clone()),
            Fatal::Forbidden(m) => Problem::Refused(m.clone()),
            other => Problem::Failed(other.to_string()),
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Problem::LoginNeeded => "Sign in again",
            Problem::Update(_) => "Update robocode-s",
            Problem::Refused(_) => "The rumble refuses this account",
            Problem::Failed(_) => "Contributing stopped",
        }
    }

    pub fn text(&self) -> String {
        match self {
            Problem::LoginNeeded => {
                "The rumble doesn't know this sign-in (any more). Sign in with GitHub again.".into()
            }
            Problem::Update(m) | Problem::Refused(m) | Problem::Failed(m) => m.clone(),
        }
    }
}

/// A line of the log, newest last.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub text: String,
    pub error: bool,
}

/// What the page shows of a contributing session. The events and the thread's messages go
/// through [`Model::apply`] and friends; nothing here touches a window or a thread.
#[derive(Debug)]
pub struct Model {
    pub phase: Phase,
    pub info: Option<RunInfo>,
    pub battles: Vec<Running>,
    pub totals: Totals,
    pub all_time: Option<AllTime>,
    /// The last results, newest last.
    pub results: VecDeque<String>,
    pub log: VecDeque<LogLine>,
    /// Nothing to do until then, and why.
    pub waiting: Option<(Instant, String)>,
    pub problem: Option<Problem>,
    started: Instant,
}

impl Model {
    pub fn new(now: Instant) -> Self {
        Self {
            phase: Phase::Idle,
            info: None,
            battles: Vec::new(),
            totals: Totals::default(),
            all_time: None,
            results: VecDeque::new(),
            log: VecDeque::new(),
            waiting: None,
            problem: None,
            started: now,
        }
    }

    /// A new session begins: the start checks run. The log stays, with a mark.
    pub fn begin(&mut self, now: Instant) {
        self.phase = Phase::Starting;
        self.info = None;
        self.battles.clear();
        self.totals = Totals::default();
        self.results.clear();
        self.waiting = None;
        self.problem = None;
        self.started = now;
        self.note(now, "starting", false);
    }

    /// The Stop button: the first press stops gently, the second abandons the running
    /// battles. Returns whether it did anything.
    pub fn request_stop(&mut self, stop: &Stop, now: Instant) -> bool {
        match self.phase {
            Phase::Idle => false,
            Phase::Starting | Phase::Running => {
                stop.stop();
                self.phase = Phase::Stopping;
                self.waiting = None;
                self.note(
                    now,
                    "stopping: the running battles finish and their results are posted",
                    false,
                );
                true
            }
            Phase::Stopping => {
                if stop.aborting() {
                    return false;
                }
                stop.abort();
                self.note(
                    now,
                    "stopping now: the running battles are abandoned",
                    false,
                );
                true
            }
        }
    }

    /// The start checks passed.
    pub fn started(&mut self, info: RunInfo, now: Instant) {
        self.note(
            now,
            &format!(
                "{} ({:?}) contributes {} on {} parallel battles; robots run on {} in sandbox {}",
                info.login,
                info.tier,
                info.divisions
                    .iter()
                    .map(Division::to_string)
                    .collect::<Vec<_>>()
                    .join(" and "),
                info.slots,
                info.platform,
                info.sandbox
            ),
            false,
        );
        self.info = Some(info);
        // A stop asked for during the checks stays asked for.
        if self.phase == Phase::Starting {
            self.phase = Phase::Running;
        }
    }

    /// The start checks refused.
    pub fn start_failed(&mut self, fatal: &Fatal, now: Instant) {
        self.phase = Phase::Idle;
        self.problem = Some(Problem::of(fatal));
        self.note(now, &fatal.to_string(), true);
    }

    /// The loop ended.
    pub fn finished(&mut self, summary: &Summary, now: Instant) {
        self.phase = Phase::Idle;
        self.battles.clear();
        self.waiting = None;
        self.problem = summary.fatal.as_ref().map(Problem::of);
        if let Some(f) = &summary.fatal {
            self.note(now, &f.to_string(), true);
        }
        self.note(
            now,
            &format!(
                "done: {} battles run ({} robot failures, {} not run, {} errors); {} batches \
                 posted, {} recordings uploaded",
                summary.battles_ok,
                summary.robot_failed,
                summary.not_run,
                summary.errors,
                summary.batches_posted,
                summary.uploaded
            ),
            false,
        );
    }

    fn note(&mut self, now: Instant, text: &str, error: bool) {
        let s = now.saturating_duration_since(self.started).as_secs();
        self.log.push_back(LogLine {
            text: format!("{:02}:{:02}  {text}", s / 60, s % 60),
            error,
        });
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    fn result(&mut self, line: String) {
        self.results.push_back(line);
        while self.results.len() > RESULT_LINES {
            self.results.pop_front();
        }
    }

    /// One event of the loop.
    pub fn apply(&mut self, event: Event, now: Instant) {
        if !matches!(event, Event::Waiting { .. }) {
            self.waiting = None;
        }
        match event {
            Event::Batch {
                batch,
                division,
                battles,
                ..
            } => self.note(
                now,
                &format!("batch {batch}: {battles} {division} battles"),
                false,
            ),
            Event::BattleStarted {
                batch,
                index,
                division,
                bots,
            } => self.battles.push(Running {
                batch,
                index,
                division,
                bots,
                round: 0,
                of: 0,
            }),
            Event::Round {
                batch,
                index,
                n,
                of,
            } => {
                if let Some(b) = self
                    .battles
                    .iter_mut()
                    .find(|b| b.batch == batch && b.index == index)
                {
                    b.round = n;
                    b.of = of;
                }
            }
            Event::BattleDone {
                ref batch,
                index,
                status,
                ..
            } => {
                self.battles
                    .retain(|b| !(b.batch == *batch && b.index == index));
                match status {
                    BattleStatus::Ok => self.totals.battles_ok += 1,
                    BattleStatus::RobotFailed => self.totals.robot_failed += 1,
                    BattleStatus::Error => self.totals.errors += 1,
                    BattleStatus::NotRun => self.totals.not_run += 1,
                }
                if let Some(line) = result_line(&event) {
                    let bad = matches!(status, BattleStatus::Error);
                    self.result(line.clone());
                    if status != BattleStatus::Ok {
                        self.note(now, &line, bad);
                    }
                }
            }
            Event::Posted {
                batch,
                accepted,
                rejected,
                duplicate,
                battles,
                reputation,
                tier,
            } => {
                self.totals.batches_posted += 1;
                self.totals.accepted += accepted as u32;
                self.totals.rejected += rejected as u32;
                self.all_time = Some(AllTime {
                    battles,
                    reputation,
                    tier,
                });
                let line = format!(
                    "posted {batch}: {accepted} accepted, {rejected} rejected{}",
                    if duplicate { ", seen before" } else { "" }
                );
                self.result(line.clone());
                self.note(now, &line, false);
            }
            Event::Uploaded { batch, n } => {
                self.totals.uploaded += n as u32;
                self.note(now, &format!("uploaded {n} recordings of {batch}"), false);
            }
            Event::Waiting { seconds, why } => {
                self.note(now, &format!("waiting {seconds} s: {why}"), false);
                self.waiting = Some((now + Duration::from_secs(seconds), why));
            }
            Event::Error { text } => self.note(now, &format!("error: {text}"), true),
        }
    }
}

/// The line for a finished battle, if it is worth one (a battle that was only stopped
/// isn't).
pub fn result_line(event: &Event) -> Option<String> {
    let Event::BattleDone {
        division,
        bots,
        status,
        scores,
        ranks,
        duration_ms,
        detail,
        ..
    } = event
    else {
        return None;
    };
    let seconds = duration_ms.map(|ms| format!(" ({:.0} s)", ms as f64 / 1000.0));
    let who = match bots.len() {
        0 | 1 => bots.join(""),
        2 => format!("{} vs {}", bots[0], bots[1]),
        n => format!("{} + {} others", bots[0], n - 1),
    };
    let outcome = match status {
        BattleStatus::Ok if bots.len() == 2 && scores.len() == 2 => {
            let total = scores[0] + scores[1];
            let share = if total > 0.0 {
                100.0 * scores[0] / total
            } else {
                50.0
            };
            format!("{share:.1}%")
        }
        BattleStatus::Ok => format!(
            "rank {} of {}",
            ranks.first().copied().unwrap_or(0),
            bots.len()
        ),
        BattleStatus::RobotFailed => {
            format!("robot failed ({})", detail.as_deref().unwrap_or("?"))
        }
        BattleStatus::Error => format!("could not run ({})", detail.as_deref().unwrap_or("?")),
        BattleStatus::NotRun => match detail.as_deref() {
            Some("stopped") | None => return None,
            Some(d) => format!("not run ({d})"),
        },
    };
    Some(format!(
        "{division} {who}: {outcome}{}",
        seconds.unwrap_or_default()
    ))
}

/// "2:05" for a countdown.
pub fn countdown(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

// ---- the account ----

enum Account {
    /// Looking at the token and asking who it is.
    Checking,
    SignedOut {
        note: Option<String>,
    },
    SigningIn {
        id: u64,
        cancel: Arc<AtomicBool>,
        code: Option<DeviceCode>,
    },
    SignedIn {
        login: String,
        me: Option<Me>,
        /// The token comes from `ROBOCODES_RUMBLE_TOKEN`: there is nothing to remove.
        from_env: bool,
        warning: Option<String>,
    },
    SigningOut,
}

/// What the threads say to the page.
enum Msg {
    Me(Result<Me, http::Error>),
    /// Asking who the user is failed in a way worth retrying; the thread asks again after `wait`.
    MeAgain(http::Error, Duration),
    Code(u64, DeviceCode),
    LoggedIn(u64, Result<Me, String>),
    SignedOut(Option<String>),
    Started(RunInfo),
    Event(Event),
    Finished(Summary),
    StartFailed(Fatal),
}

/// Opens `url` in the browser. The window's own link opening is not built in, so this asks
/// the system.
pub fn open_url(url: &str) -> Result<(), String> {
    if clean_server(url).is_none() {
        return Err(format!("not a web address: {url}"));
    }
    let attempts: &[(&str, &[&str])] = if cfg!(windows) {
        &[("rundll32", &["url.dll,FileProtocolHandler"])]
    } else if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else {
        &[
            ("xdg-open", &[]),
            ("wslview", &[]),
            ("sensible-browser", &[]),
        ]
    };
    let mut last = String::new();
    for (program, args) in attempts {
        match std::process::Command::new(program)
            .args(*args)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Reaped, so that it doesn't stay a zombie.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(e) => last = format!("{program}: {e}"),
        }
    }
    Err(last)
}

fn machine_name() -> String {
    for var in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(name) = std::env::var(var)
            && !name.trim().is_empty()
        {
            return name.trim().to_string();
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "robocode-s".to_string())
}

/// Sleeps `d` in short steps, ending early once `cancel` is set (the login's polls wait
/// seconds).
fn sleep_unless(cancel: &AtomicBool, d: Duration) {
    let step = Duration::from_millis(100);
    let mut left = d;
    while !left.is_zero() && !cancel.load(Ordering::SeqCst) {
        let s = left.min(step);
        std::thread::sleep(s);
        left -= s;
    }
}

// ---- the page ----

/// The Run Rumble page: the account, the contributing thread and what it reported.
pub struct Rumble {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub model: Model,
    account: Account,
    /// The running session's stop, while there is a thread.
    stop: Option<Stop>,
    next_login: u64,
    looked: bool,
    /// Stops the thread that keeps asking the rumble who the user is.
    asking: Arc<AtomicBool>,
    /// Asking the user to agree to running robots without a sandbox.
    consent: bool,
    /// The window was asked to close while contributing: since when.
    closing: Option<Instant>,
    /// Why something the page started couldn't be done.
    note: Option<String>,
    /// The user asked to update robocode-s (a problem's button): the app takes it.
    pub update_asked: bool,
}

impl Default for Rumble {
    fn default() -> Self {
        Self::new()
    }
}

impl Rumble {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            model: Model::new(Instant::now()),
            account: Account::Checking,
            stop: None,
            next_login: 0,
            looked: false,
            asking: Arc::new(AtomicBool::new(false)),
            consent: false,
            closing: None,
            note: None,
            update_asked: false,
        }
    }

    /// Whether a contributor thread is alive.
    pub fn active(&self) -> bool {
        self.stop.is_some()
    }

    fn send(tx: &Sender<Msg>, ctx: &egui::Context, msg: Msg) {
        let _ = tx.send(msg);
        ctx.request_repaint();
    }

    /// Takes the threads' messages. Call it every frame, whatever page is open.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg, Instant::now());
        }
        if self.model.waiting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn handle(&mut self, msg: Msg, now: Instant) {
        match msg {
            Msg::Me(Ok(me)) => {
                if let Account::SignedIn {
                    login,
                    me: slot,
                    warning,
                    ..
                } = &mut self.account
                {
                    *login = me.login.clone();
                    *warning = None;
                    *slot = Some(me);
                }
            }
            Msg::Me(Err(http::Error::Unauthorized(_))) => {
                self.account = Account::SignedOut {
                    note: Some("The saved sign-in is not valid any more.".into()),
                };
            }
            Msg::Me(Err(e)) => {
                if let Account::SignedIn { warning, .. } = &mut self.account {
                    *warning = Some(format!("Could not ask the rumble who you are: {e}"));
                }
            }
            Msg::MeAgain(e, wait) => {
                if let Account::SignedIn { warning, .. } = &mut self.account {
                    *warning = Some(format!(
                        "Could not ask the rumble who you are: {e}. Trying again in {} s.",
                        wait.as_secs().max(1)
                    ));
                }
            }
            Msg::Code(id, code) => {
                if let Account::SigningIn {
                    id: mine,
                    code: slot,
                    ..
                } = &mut self.account
                    && *mine == id
                {
                    // Opened for the user once; the page has a button for again.
                    let _ = open_url(&code.verification_uri);
                    *slot = Some(code);
                }
            }
            Msg::LoggedIn(id, answer) => {
                if matches!(&self.account, Account::SigningIn { id: mine, .. } if *mine == id) {
                    self.account = match answer {
                        Ok(me) => Account::SignedIn {
                            login: me.login.clone(),
                            me: Some(me),
                            from_env: false,
                            warning: None,
                        },
                        Err(note) => Account::SignedOut { note: Some(note) },
                    };
                }
            }
            Msg::SignedOut(warning) => {
                self.account = Account::SignedOut { note: warning };
            }
            Msg::Started(info) => self.model.started(info, now),
            Msg::Event(event) => self.model.apply(event, now),
            Msg::Finished(summary) => {
                self.model.finished(&summary, now);
                self.stop = None;
                if matches!(summary.fatal, Some(Fatal::LoginNeeded)) {
                    self.account = Account::SignedOut {
                        note: Some("The rumble doesn't know the saved sign-in.".into()),
                    };
                }
            }
            Msg::StartFailed(fatal) => {
                self.model.start_failed(&fatal, now);
                self.stop = None;
                if matches!(fatal, Fatal::LoginNeeded) {
                    self.account = Account::SignedOut {
                        note: Some("The rumble doesn't know the saved sign-in.".into()),
                    };
                }
            }
        }
    }

    /// Reads the saved sign-in the first time the page is shown, and asks who it is.
    fn look(&mut self, ctx: &egui::Context, settings: &RumbleSettings) {
        if self.looked {
            return;
        }
        self.looked = true;
        match tokenfile::resolve() {
            Ok(None) => self.account = Account::SignedOut { note: None },
            Err(e) => {
                self.account = Account::SignedOut {
                    note: Some(format!("The saved sign-in can't be read: {e}")),
                }
            }
            Ok(Some(creds)) => {
                self.account = Account::SignedIn {
                    login: creds.login.clone().unwrap_or_default(),
                    me: None,
                    from_env: creds.source == tokenfile::Source::Env,
                    warning: None,
                };
                let server = server_now(settings, creds.server.as_deref());
                let (tx, ctx) = (self.tx.clone(), ctx.clone());
                let cancel = Arc::new(AtomicBool::new(false));
                self.asking = cancel.clone();
                std::thread::spawn(move || {
                    // Network trouble (a name lookup that fails while Windows wakes up, say)
                    // passes: keep asking, backing off, until there is an answer.
                    let api = api_for(&server, Some(creds.token));
                    let mut backoff = Backoff::network();
                    loop {
                        let me = api.me();
                        if cancel.load(Ordering::SeqCst) {
                            return;
                        }
                        match me {
                            Err(e) if e.is_retryable() => {
                                let wait = match e.retry_after() {
                                    Some(after) => {
                                        backoff::retry_after(after, backoff::random_unit())
                                    }
                                    None => backoff.next(backoff::random_unit()),
                                };
                                Self::send(&tx, &ctx, Msg::MeAgain(e, wait));
                                sleep_unless(&cancel, wait);
                                if cancel.load(Ordering::SeqCst) {
                                    return;
                                }
                            }
                            me => {
                                Self::send(&tx, &ctx, Msg::Me(me));
                                return;
                            }
                        }
                    }
                });
            }
        }
    }

    fn sign_in(&mut self, ctx: &egui::Context, settings: &RumbleSettings) {
        self.asking.store(true, Ordering::SeqCst);
        let Some(path) = tokenfile::path() else {
            self.account = Account::SignedOut {
                note: Some(
                    "There is no settings folder to keep the sign-in in (set \
                     ROBOCODES_RUMBLE_CONFIG to a file)."
                        .into(),
                ),
            };
            return;
        };
        let id = self.next_login;
        self.next_login += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.account = Account::SigningIn {
            id,
            cancel: cancel.clone(),
            code: None,
        };
        let server = server_now(settings, None);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let api = api_for(&server, None);
            let github = std::env::var(GITHUB_VAR)
                .ok()
                .filter(|g| !g.is_empty())
                .unwrap_or_else(|| GITHUB.to_string());
            let (sleeping, asked) = (cancel.clone(), cancel);
            let mut flow = DeviceFlow {
                github,
                sleep: Box::new(move |d| sleep_unless(&sleeping, d)),
                cancelled: Box::new(move || asked.load(Ordering::SeqCst)),
            };
            let mut show = |code: &DeviceCode| {
                Self::send(&tx, &ctx, Msg::Code(id, code.clone()));
            };
            let answer = flow
                .login(&api, &machine_name(), &mut show)
                .map_err(|e| e.to_string())
                .and_then(|answer| {
                    tokenfile::write(
                        &path,
                        &TokenFile {
                            server: server.clone(),
                            token: answer.token,
                            login: answer.user.login.clone(),
                            saved_at: tokenfile::now_iso(),
                        },
                    )
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                    Ok(answer.user)
                });
            Self::send(&tx, &ctx, Msg::LoggedIn(id, answer));
        });
    }

    /// Revokes the token on the server and removes the file.
    fn sign_out(&mut self, ctx: &egui::Context, settings: &RumbleSettings) {
        self.asking.store(true, Ordering::SeqCst);
        self.account = Account::SigningOut;
        let path = tokenfile::path();
        let file = path
            .as_ref()
            .and_then(|p| tokenfile::read(p).ok().flatten());
        let server = server_now(settings, file.as_ref().map(|f| f.server.as_str()));
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut warning = None;
            if let Some(file) = &file
                && let Err(e) = api_for(&server, Some(file.token.clone())).revoke_token("current")
            {
                warning = Some(format!(
                    "Signed out here, but the token could not be revoked on {server} ({e}); \
                     revoke it on your account page."
                ));
            }
            if let Some(path) = &path
                && let Err(e) = tokenfile::remove(path)
            {
                warning = Some(format!("{}: {e}", path.display()));
            }
            Self::send(&tx, &ctx, Msg::SignedOut(warning));
        });
    }

    /// Starts contributing on a thread of its own.
    fn start(&mut self, ctx: &egui::Context, settings: &Settings, env: &Environment) {
        let rumble = &settings.rumble;
        let creds: Credentials = match tokenfile::resolve() {
            Ok(Some(c)) => c,
            _ => {
                self.account = Account::SignedOut {
                    note: Some("Sign in first.".into()),
                };
                return;
            }
        };
        let server = server_now(rumble, creds.server.as_deref());
        let slots = chosen_slots(rumble);
        let sandbox = settings.sandbox();
        let mut options = BattleOptions::new(env.clone());
        // The rumble's battles take the robots of the battle's own jars only.
        options.env.jars.clear();
        options.env.sandbox = sandbox;
        options.slots = slots;
        options.allow_unsandboxed = sandbox == Sandbox::None && rumble.unsandboxed_consent;
        let mut config = Config::new(&server, &creds.token, options);
        config.divisions = rumble.divisions();
        let stop = Stop::new();
        self.stop = Some(stop.clone());
        self.model.begin(Instant::now());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("rumble".into())
            .spawn(move || {
                let contributor = match Contributor::new(config) {
                    Ok(c) => c,
                    Err(fatal) => return Self::send(&tx, &ctx, Msg::StartFailed(fatal)),
                };
                let info = RunInfo {
                    login: contributor.me().login.clone(),
                    tier: contributor.me().tier,
                    slots: contributor.slots(),
                    divisions: contributor.divisions().to_vec(),
                    platform: contributor.platform().to_string(),
                    sandbox: sandbox.name().to_string(),
                };
                Self::send(&tx, &ctx, Msg::Started(info));
                let summary = contributor.run(&stop, |event| {
                    Self::send(&tx, &ctx, Msg::Event(event));
                });
                Self::send(&tx, &ctx, Msg::Finished(summary));
            });
        if let Err(e) = spawned {
            self.stop = None;
            self.model.phase = Phase::Idle;
            self.model.problem = Some(Problem::Failed(format!("can't start a thread: {e}")));
        }
    }

    /// The Stop button.
    fn stop_pressed(&mut self) {
        if let Some(stop) = &self.stop {
            self.model.request_stop(stop, Instant::now());
        }
    }

    /// Closing the window while contributing: stop gently, wait for the posts (at most
    /// [`CLOSE_WAIT`]), then close. Returns after cancelling the first close request; a
    /// second one (or the Quit now button) closes at once. Call it every frame.
    pub fn closing(&mut self, ctx: &egui::Context) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        if requested && self.active() && self.closing.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = Some(Instant::now());
            if self.model.phase != Phase::Stopping {
                self.stop_pressed();
            }
        }
        let Some(since) = self.closing else {
            return;
        };
        if !self.active() {
            self.closing = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let mut quit = since.elapsed() >= CLOSE_WAIT;
        egui::Window::new("Quitting")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Posting the finished battles to the rumble...");
                });
                ui.label(
                    RichText::new(
                        "Battles still running are abandoned; what can't be posted now waits \
                         for the next start.",
                    )
                    .weak(),
                );
                quit |= ui.button("Quit now").clicked();
            });
        if quit {
            if let Some(stop) = &self.stop {
                stop.abort();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// The sandbox consent dialog (§13.1): returns true once agreed.
    fn consent_window(&mut self, ctx: &egui::Context) -> bool {
        if !self.consent {
            return false;
        }
        let mut agreed = false;
        let mut cancel = false;
        egui::Window::new("Run robots without a sandbox?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_width(460.0)
            .show(ctx, |ui| {
                ui.add(
                    egui::Label::new(
                        "The rumble runs robots written by anyone. With the sandbox set to \
                         \"none\" they run as plain processes with all your rights: they \
                         could read your files or use your network.",
                    )
                    .wrap(),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Choose another sandbox in Options (os, bwrap, docker or podman) unless \
                             this computer is one you don't mind risking. Your answer is \
                             remembered.",
                        )
                        .weak(),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let run =
                        egui::Button::new(RichText::new("Run unsandboxed").color(Color32::BLACK))
                            .fill(Color32::from_rgb(0xe0, 0x9a, 0x4a));
                    agreed = ui.add(run).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if agreed || cancel {
            self.consent = false;
        }
        agreed
    }

    // ---- drawing ----

    /// The page's content, inside the shell's column. Returns whether the options changed.
    pub fn ui(&mut self, ui: &mut egui::Ui, settings: &mut Settings, env: &Environment) -> bool {
        let ctx = ui.ctx().clone();
        self.look(&ctx, &settings.rumble);
        let before = settings.rumble.clone();
        let server = server_now(&settings.rumble, None);
        let mut start = false;

        ui.heading("Run Rumble");
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Lend this computer to the robocode-s rumble, a continuously running \
                     league of robots: it runs the battles the rumble hands out and posts the \
                     results. Rankings and replays are on the rumble's website.",
                )
                .weak(),
            )
            .wrap(),
        );
        if ui.link(server.as_str()).clicked() {
            self.note = open_url(&server).err();
        }

        self.problem_ui(ui, &server);

        section(ui, "Account");
        self.account_ui(ui, &ctx, settings);

        section(ui, "Contribute");
        start |= self.contribute_ui(ui, settings);

        let shown = self.model.phase != Phase::Idle
            || !self.model.results.is_empty()
            || !self.model.log.is_empty();
        if shown {
            section(ui, "Now");
            self.now_ui(ui);
            section(ui, "Totals");
            self.totals_ui(ui);
            if !self.model.results.is_empty() {
                section(ui, "Last results");
                self.results_ui(ui);
            }
            section(ui, "Log");
            self.log_ui(ui);
        }
        if let Some(note) = &self.note {
            ui.add_space(8.0);
            ui.colored_label(Color32::LIGHT_RED, note);
        }

        // The sandbox "none" asks once.
        if start {
            if settings.sandbox() == Sandbox::None && !settings.rumble.unsandboxed_consent {
                self.consent = true;
            } else {
                self.start(&ctx, settings, env);
            }
        }
        if self.consent_window(&ctx) {
            settings.rumble.unsandboxed_consent = true;
            self.start(&ctx, settings, env);
        }
        settings.rumble != before
    }

    fn problem_ui(&mut self, ui: &mut egui::Ui, server: &str) {
        let Some(problem) = self.model.problem.clone() else {
            return;
        };
        ui.add_space(12.0);
        let red = Color32::from_rgb(0xe0, 0x6c, 0x6c);
        egui::Frame::NONE
            .fill(shell::BAR)
            .stroke(egui::Stroke::new(1.0, red))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(problem.title()).strong().color(red));
                ui.add(egui::Label::new(problem.text()).wrap());
                if let Problem::Update(_) = problem {
                    ui.add_space(4.0);
                    let page = format!("{server}/download");
                    ui.horizontal(|ui| {
                        if ui.button("Update robocode-s").clicked() {
                            self.update_asked = true;
                        }
                        if ui.button("Open the download page").clicked() {
                            self.note = open_url(&page).err();
                        }
                    });
                    ui.label(RichText::new(page).weak().small());
                }
            });
    }

    fn account_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, settings: &mut Settings) {
        let busy = self.model.phase != Phase::Idle;
        let (mut sign_in, mut sign_out, mut cancel) = (false, false, false);
        let mut open = None;
        match &self.account {
            Account::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for your sign-in...");
                });
            }
            Account::SigningOut => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Signing out...");
                });
            }
            Account::SignedOut { note } => {
                ui.add(
                    egui::Label::new(
                        "Not signed in. The rumble knows contributors by their GitHub account \
                         (your public profile only).",
                    )
                    .wrap(),
                );
                if let Some(note) = note {
                    ui.colored_label(Color32::LIGHT_RED, note);
                }
                ui.add_space(6.0);
                sign_in = ui
                    .add(egui::Button::new("Sign in with GitHub").min_size(egui::vec2(170.0, 30.0)))
                    .clicked();
            }
            Account::SigningIn { code: None, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Asking GitHub for a code...");
                    cancel = ui.button("Cancel").clicked();
                });
            }
            Account::SigningIn {
                code: Some(code), ..
            } => {
                ui.label(format!(
                    "Open {} and enter this code:",
                    code.verification_uri
                ));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(&code.user_code)
                        .size(34.0)
                        .monospace()
                        .strong()
                        .color(shell::ACCENT),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(code.user_code.clone());
                    }
                    if ui.button("Open GitHub").clicked() {
                        open = Some(code.verification_uri.clone());
                    }
                    cancel = ui.button("Cancel").clicked();
                    ui.spinner();
                    ui.label(RichText::new("Waiting for you to confirm it...").weak());
                });
            }
            Account::SignedIn {
                login,
                me,
                from_env,
                warning,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Signed in as");
                    ui.label(RichText::new(login).strong().color(shell::ACCENT));
                    let out = ui.add_enabled(!busy && !from_env, egui::Button::new("Sign out"));
                    sign_out = if *from_env {
                        out.on_disabled_hover_text(
                            "The sign-in comes from ROBOCODES_RUMBLE_TOKEN: unset it instead",
                        )
                        .clicked()
                    } else if busy {
                        out.on_disabled_hover_text("Stop contributing first")
                            .clicked()
                    } else {
                        out.on_hover_text("Revoke this computer's token and forget it")
                            .clicked()
                    };
                });
                if let Some(me) = me {
                    options_grid(ui, "rumble-me", |ui| {
                        ui.label("Tier");
                        ui.label(tier_name(me.tier));
                        ui.end_row();
                        ui.label("Reputation");
                        ui.label(me.reputation.to_string());
                        ui.end_row();
                        ui.label("Battles");
                        ui.label(me.battles.to_string());
                        ui.end_row();
                    });
                }
                if let Some(w) = warning {
                    ui.colored_label(Color32::LIGHT_RED, w);
                }
            }
        }
        if sign_in {
            self.sign_in(ctx, &settings.rumble);
        }
        if sign_out {
            self.sign_out(ctx, &settings.rumble);
        }
        if cancel {
            if let Account::SigningIn { cancel, .. } = &self.account {
                cancel.store(true, Ordering::SeqCst);
            }
            self.account = Account::SignedOut { note: None };
        }
        if let Some(url) = open {
            self.note = open_url(&url).err();
        }

        ui.add_space(8.0);
        egui::CollapsingHeader::new("Advanced")
            .default_open(false)
            .show(ui, |ui| {
                let from_env = std::env::var(SERVER_VAR)
                    .ok()
                    .filter(|v| clean_server(v).is_some());
                options_grid(ui, "rumble-server", |ui| {
                    ui.label("Server");
                    ui.add_enabled(
                        !busy,
                        egui::TextEdit::singleline(&mut settings.rumble.server)
                            .hint_text(DEFAULT_SERVER)
                            .desired_width(340.0),
                    );
                    ui.end_row();
                    if let Some(e) = from_env {
                        ui.label("");
                        ui.label(
                            RichText::new(format!("{SERVER_VAR} is set and wins: {e}"))
                                .weak()
                                .small(),
                        );
                        ui.end_row();
                    }
                });
            });
    }

    /// The contribute switch and its options; returns whether Start was pressed.
    fn contribute_ui(&mut self, ui: &mut egui::Ui, settings: &mut Settings) -> bool {
        let phase = self.model.phase;
        let signed_in = matches!(self.account, Account::SignedIn { .. });
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let sandbox = settings.sandbox();
        let slots = chosen_slots(&settings.rumble);
        let offered = offered_divisions(&settings.rumble.divisions(), slots);
        let (mut start, mut stop) = (false, false);

        options_grid(ui, "rumble-contribute", |ui| {
            ui.label("Contribute");
            ui.vertical(|ui| {
                ui.horizontal(|ui| match phase {
                    Phase::Idle => {
                        let ready = signed_in && !offered.is_empty();
                        let button = egui::Button::new(
                            RichText::new("Start contributing")
                                .size(16.0)
                                .strong()
                                .color(shell::BAR),
                        )
                        .fill(shell::ACCENT)
                        .min_size(egui::vec2(190.0, 36.0));
                        let r = ui.add_enabled(ready, button);
                        let r = if !signed_in {
                            r.on_disabled_hover_text("Sign in first")
                        } else if offered.is_empty() {
                            r.on_disabled_hover_text("Choose a division the parallel battles fit")
                        } else {
                            r
                        };
                        start = r.clicked();
                    }
                    Phase::Starting | Phase::Running => {
                        let r = ui.add(
                            egui::Button::new(RichText::new("Stop").size(16.0).strong())
                                .min_size(egui::vec2(190.0, 36.0)),
                        );
                        stop = r
                            .on_hover_text(
                                "No new battles; the running ones finish and their results are \
                             posted",
                            )
                            .clicked();
                        if phase == Phase::Starting {
                            ui.spinner();
                            ui.label(
                                RichText::new("Checking the rumble and this machine...").weak(),
                            );
                        } else {
                            ui.label(RichText::new("Contributing").color(shell::ACCENT));
                        }
                    }
                    Phase::Stopping => {
                        let r = ui.add(
                            egui::Button::new(RichText::new("Stop now").size(16.0).strong())
                                .min_size(egui::vec2(190.0, 36.0)),
                        );
                        stop = r
                            .on_hover_text("Abandon the running battles (they are not posted)")
                            .clicked();
                        ui.spinner();
                        ui.label(
                            RichText::new("Finishing the running battles and posting them...")
                                .weak(),
                        );
                    }
                });
                ui.add_space(12.0);
            });
            ui.end_row();

            ui.add_enabled_ui(phase == Phase::Idle, |ui| {
                ui.label("Parallel battles").on_hover_text(
                    "Battles at once: a 1v1 battle takes one, a melee battle five. Each robot \
                     is a JVM of about 512 MB, so the default is half the cores and one per \
                     2 GiB of memory.",
                );
            });
            ui.add_enabled_ui(phase == Phase::Idle, |ui| {
                ui.horizontal(|ui| {
                    let mut n = slots.clamp(1, cores.max(1));
                    let slider = egui::Slider::new(&mut n, 1..=cores.max(1));
                    if ui.add(slider).changed() {
                        settings.rumble.parallel = Some(n);
                    }
                    if ui
                        .add_enabled(
                            settings.rumble.parallel.is_some(),
                            egui::Button::new("Default"),
                        )
                        .on_hover_text(format!(
                            "{} (half the cores, and the memory)",
                            default_slots()
                        ))
                        .clicked()
                    {
                        settings.rumble.parallel = None;
                    }
                });
            });
            ui.end_row();

            ui.label("Divisions");
            ui.add_enabled_ui(phase == Phase::Idle, |ui| {
                ui.horizontal(|ui| {
                    for division in Division::ALL {
                        let mut on = settings.rumble.divisions().contains(&division);
                        let name = match division {
                            Division::OneVsOne => "1v1",
                            Division::Melee => "Melee",
                        };
                        if ui.checkbox(&mut on, name).changed() {
                            settings.rumble.set_division(division, on);
                        }
                    }
                });
            });
            ui.end_row();
            if let Some(why) = divisions_note(&settings.rumble.divisions(), slots, cores) {
                ui.label("");
                ui.add(
                    egui::Label::new(RichText::new(why).color(Color32::from_rgb(0xe0, 0xb0, 0x5a)))
                        .wrap(),
                );
                ui.end_row();
            }

            ui.label("Sandbox");
            ui.horizontal(|ui| {
                ui.label(RichText::new(sandbox.name()).strong());
                ui.label(RichText::new("(set in Options)").weak());
            });
            ui.end_row();
            if sandbox == Sandbox::None {
                ui.label("");
                let text = if settings.rumble.unsandboxed_consent {
                    "Robots from anyone run as plain processes with all your rights (you agreed)."
                } else {
                    "Robots from anyone would run as plain processes with all your rights: \
                     you are asked before the first start."
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(text).color(Color32::from_rgb(0xe0, 0x6c, 0x6c)),
                    )
                    .wrap(),
                );
                ui.end_row();
            }
        });
        if stop {
            self.stop_pressed();
        }
        start
    }

    fn now_ui(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        if let Some(info) = &self.model.info {
            let divisions: Vec<String> = info.divisions.iter().map(Division::to_string).collect();
            ui.label(
                RichText::new(format!(
                    "{} ({}): {} on {} parallel battles, robots on {} in sandbox {}",
                    info.login,
                    tier_name(info.tier),
                    divisions.join(" and "),
                    info.slots,
                    info.platform,
                    info.sandbox
                ))
                .weak(),
            );
        }
        if let Some((until, why)) = &self.model.waiting {
            let left = until.saturating_duration_since(now).as_secs() + 1;
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Waiting {}: {why}", countdown(left)));
            });
        }
        ui.add_space(4.0);
        if self.model.battles.is_empty() {
            let text = match self.model.phase {
                Phase::Idle => "Not contributing.",
                Phase::Starting => "Starting...",
                Phase::Stopping => "No battle running.",
                Phase::Running if self.model.waiting.is_some() => "No battle running.",
                Phase::Running => "Getting work...",
            };
            ui.label(RichText::new(text).weak());
        }
        // Not a grid: its columns follow the robot names, so the bars would jump around as
        // battles come and go. The bars stay on the right edge and the names truncate instead.
        ui.spacing_mut().item_spacing.y = 6.0;
        let division_width = Division::ALL
            .iter()
            .map(|d| text_width(ui, &d.to_string()))
            .fold(0.0, f32::max);
        for b in &self.model.battles {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let fraction = if b.of == 0 {
                    0.0
                } else {
                    b.round as f32 / b.of as f32
                };
                let text = if b.of == 0 {
                    "starting".to_string()
                } else {
                    format!("round {} of {}", b.round, b.of)
                };
                let division = ui.label(RichText::new(b.division.to_string()).weak());
                ui.add_space((division_width - division.rect.width()).max(0.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    progress(ui, fraction, &text);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(battle_title(&b.bots)).truncate());
                    });
                });
            });
        }
    }

    fn totals_ui(&self, ui: &mut egui::Ui) {
        let t = &self.model.totals;
        let all_time = self.model.all_time.or_else(|| match &self.account {
            Account::SignedIn { me: Some(me), .. } => Some(AllTime::of(me)),
            _ => None,
        });
        options_grid(ui, "rumble-totals", |ui| {
            ui.label("This session");
            ui.label(format!(
                "{} battles run, {} robot failures, {} not run, {} errors",
                t.battles_ok, t.robot_failed, t.not_run, t.errors
            ));
            ui.end_row();
            ui.label("Posted");
            ui.label(format!(
                "{} batches ({} battles accepted, {} rejected), {} recordings uploaded",
                t.batches_posted, t.accepted, t.rejected, t.uploaded
            ));
            ui.end_row();
            ui.label("All time");
            ui.label(match all_time {
                Some(a) => format!(
                    "{} battles, reputation {}, tier {}",
                    a.battles,
                    a.reputation,
                    tier_name(a.tier)
                ),
                None => "-".into(),
            });
            ui.end_row();
        });
    }

    /// The last results in a scrolling list like the log: newest at the bottom.
    fn results_ui(&self, ui: &mut egui::Ui) {
        list_frame(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("rumble-results")
                .auto_shrink([false, false])
                .min_scrolled_height(150.0)
                .max_height(150.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for line in &self.model.results {
                        ui.label(RichText::new(line).monospace().small());
                    }
                });
        });
    }

    fn log_ui(&self, ui: &mut egui::Ui) {
        list_frame(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("rumble-log")
                .auto_shrink([false, false])
                .min_scrolled_height(150.0)
                .max_height(150.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for line in &self.model.log {
                        let text = RichText::new(&line.text).monospace().small();
                        if line.error {
                            ui.label(text.color(Color32::LIGHT_RED));
                        } else {
                            ui.label(text);
                        }
                    }
                });
        });
    }

    /// A canned state of the page, for screenshots (`--rumble-demo`): `out`, `code`,
    /// `signed-in`, `contributing`, `waiting` or `update`.
    pub fn demo(state: &str) -> Option<Self> {
        let mut page = Self::new();
        page.looked = true;
        let now = Instant::now();
        let me = Me {
            uid: "g1".into(),
            login: "octocat".into(),
            name: "The Octocat".into(),
            country: "NL".into(),
            reputation: 12,
            tier: Tier::New,
            verify_rate: 0.2,
            battles: 340,
            verified_matches: 17,
            mismatches: 0,
            admin: false,
            since: "2026-09-01T00:00:00Z".into(),
        };
        let signed_in = Account::SignedIn {
            login: me.login.clone(),
            me: Some(me),
            from_env: false,
            warning: None,
        };
        let info = RunInfo {
            login: "octocat".into(),
            tier: Tier::New,
            slots: 4,
            divisions: vec![Division::OneVsOne],
            platform: "JVM 21.0.4".into(),
            sandbox: "os".into(),
        };
        match state {
            "out" => page.account = Account::SignedOut { note: None },
            "code" => {
                page.account = Account::SigningIn {
                    id: 0,
                    cancel: Arc::new(AtomicBool::new(false)),
                    code: Some(DeviceCode {
                        device_code: "x".into(),
                        user_code: "WDJB-MJHT".into(),
                        verification_uri: "https://github.com/login/device".into(),
                        expires_in: 900,
                        interval: 5,
                    }),
                }
            }
            "signed-in" => page.account = signed_in,
            "update" => {
                page.account = signed_in;
                page.model.start_failed(
                    &Fatal::Engine(
                        "the rumble ranks engine version 7, this robocode-s has 6: update \
                         robocode-s"
                            .into(),
                    ),
                    now,
                );
            }
            "contributing" | "waiting" => {
                page.account = signed_in;
                let m = &mut page.model;
                m.begin(now);
                m.started(info, now);
                let bots = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
                m.apply(
                    Event::Batch {
                        batch: "b-102".into(),
                        division: Division::OneVsOne,
                        battles: 12,
                        deadline: String::new(),
                    },
                    now,
                );
                for (i, (a, b, n)) in [
                    ("abc.Walls 1.2", "xyz.Spinner 0.9", 7),
                    ("kid.Crazy 3.0", "foo.Sitter 1.0", 3),
                    ("cx.Mini 1.4", "dm.Tracker 2.1", 1),
                ]
                .into_iter()
                .enumerate()
                {
                    m.apply(
                        Event::BattleStarted {
                            batch: "b-102".into(),
                            index: i as u32,
                            division: Division::OneVsOne,
                            bots: bots(a, b),
                        },
                        now,
                    );
                    m.apply(
                        Event::Round {
                            batch: "b-102".into(),
                            index: i as u32,
                            n,
                            of: 35,
                        },
                        now,
                    );
                }
                for (i, share) in [(8, 61.3), (9, 48.2), (10, 70.1)] {
                    m.apply(
                        Event::BattleDone {
                            batch: "b-102".into(),
                            index: i,
                            division: Division::OneVsOne,
                            bots: bots("old.Bot 1.0", "new.Bot 2.0"),
                            status: BattleStatus::Ok,
                            scores: vec![share, 100.0 - share],
                            ranks: vec![1, 2],
                            duration_ms: Some(41_000),
                            detail: None,
                        },
                        now,
                    );
                }
                m.apply(
                    Event::Posted {
                        batch: "b-101".into(),
                        accepted: 12,
                        rejected: 0,
                        duplicate: false,
                        battles: 352,
                        reputation: 13,
                        tier: Tier::New,
                    },
                    now,
                );
                if state == "waiting" {
                    m.battles.clear();
                    m.apply(
                        Event::Waiting {
                            seconds: 95,
                            why: "no work: the rumble is caught up".into(),
                        },
                        now,
                    );
                }
            }
            _ => return None,
        }
        Some(page)
    }
}

/// How wide `text` is in the body font.
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, Color32::WHITE))
        .size()
        .x
}

/// A bar `fraction` full (0 to 1) with `text` centred on it.
fn progress(ui: &mut egui::Ui, fraction: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(200.0, 16.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, shell::BAR);
    let mut full = rect;
    full.set_right(rect.left() + rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(full, 3.0, shell::ACCENT.gamma_multiply(0.45));
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, shell::LINE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        ui.visuals().strong_text_color(),
    );
}

/// The parallel battles chosen:the options', else the client's default.
pub fn chosen_slots(rumble: &RumbleSettings) -> usize {
    rumble.parallel.unwrap_or_else(default_slots).max(1)
}

/// Why a chosen division can't run, for the page to say inline.
pub fn divisions_note(wanted: &[Division], slots: usize, cores: usize) -> Option<String> {
    if wanted.is_empty() {
        return Some("Choose at least one division.".into());
    }
    if wanted.contains(&Division::Melee) && slots < MELEE_SLOTS {
        let fix = if cores >= MELEE_SLOTS {
            format!("Raise the parallel battles to {MELEE_SLOTS} or more to take melee.")
        } else {
            format!("This computer has {cores} cores: melee is out of reach.")
        };
        let only = if wanted.contains(&Division::OneVsOne) {
            " Only 1v1 is offered."
        } else {
            ""
        };
        return Some(format!(
            "A melee battle takes {MELEE_SLOTS} of the parallel battles (it has {slots}). {fix}{only}"
        ));
    }
    None
}

pub fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::New => "new",
        Tier::Verifier => "verifier",
        Tier::Quarantined => "quarantined",
        Tier::Banned => "banned",
    }
}

fn battle_title(bots: &[String]) -> String {
    match bots.len() {
        0 | 1 => bots.join(""),
        2 => format!("{} vs {}", bots[0], bots[1]),
        n => format!("{} + {} others", bots[0], n - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn done(status: BattleStatus, bots: &[&str], scores: &[f64], detail: Option<&str>) -> Event {
        Event::BattleDone {
            batch: "b".into(),
            index: 0,
            division: Division::OneVsOne,
            bots: bots.iter().map(|s| s.to_string()).collect(),
            status,
            scores: scores.to_vec(),
            ranks: vec![1, 2],
            duration_ms: Some(12_400),
            detail: detail.map(str::to_string),
        }
    }

    fn started(index: u32) -> Event {
        Event::BattleStarted {
            batch: "b".into(),
            index,
            division: Division::OneVsOne,
            bots: vec!["a 1".into(), "b 1".into()],
        }
    }

    #[test]
    fn the_server_comes_from_the_environment_then_the_options_then_the_token_file() {
        assert_eq!(server_for(None, "", None), DEFAULT_SERVER);
        assert_eq!(
            server_for(None, "", Some("https://stored.example/")),
            "https://stored.example"
        );
        assert_eq!(
            server_for(None, "https://opt.example", Some("https://stored.example")),
            "https://opt.example"
        );
        assert_eq!(
            server_for(Some("http://env.example"), "https://opt.example", None),
            "http://env.example"
        );
        // Text that is no URL is passed over.
        assert_eq!(
            server_for(Some("nonsense"), " rumble ", None),
            DEFAULT_SERVER
        );
    }

    #[test]
    fn events_make_the_running_list_and_the_totals() {
        let now = t0();
        let mut m = Model::new(now);
        m.begin(now);
        assert_eq!(m.phase, Phase::Starting);
        m.apply(started(0), now);
        m.apply(started(1), now);
        m.apply(
            Event::Round {
                batch: "b".into(),
                index: 1,
                n: 4,
                of: 35,
            },
            now,
        );
        assert_eq!(m.battles.len(), 2);
        assert_eq!((m.battles[1].round, m.battles[1].of), (4, 35));
        m.apply(
            done(BattleStatus::Ok, &["a 1", "b 1"], &[60.0, 40.0], None),
            now,
        );
        assert_eq!(m.battles.len(), 1, "battle 0 is done");
        assert_eq!(m.battles[0].index, 1);
        assert_eq!(m.totals.battles_ok, 1);
        assert_eq!(m.results.back().unwrap(), "1v1 a 1 vs b 1: 60.0% (12 s)");
        m.apply(
            Event::Posted {
                batch: "b".into(),
                accepted: 3,
                rejected: 1,
                duplicate: false,
                battles: 500,
                reputation: 9,
                tier: Tier::Verifier,
            },
            now,
        );
        assert_eq!((m.totals.accepted, m.totals.rejected), (3, 1));
        assert_eq!(m.totals.batches_posted, 1);
        assert_eq!(
            m.all_time,
            Some(AllTime {
                battles: 500,
                reputation: 9,
                tier: Tier::Verifier
            })
        );
        m.apply(
            Event::Uploaded {
                batch: "b".into(),
                n: 2,
            },
            now,
        );
        assert_eq!(m.totals.uploaded, 2);
    }

    #[test]
    fn failures_count_and_a_stopped_battle_has_no_result_line() {
        let now = t0();
        let mut m = Model::new(now);
        m.apply(
            done(
                BattleStatus::RobotFailed,
                &["a 1", "b 1"],
                &[],
                Some("load"),
            ),
            now,
        );
        m.apply(
            done(BattleStatus::Error, &["a 1", "b 1"], &[], Some("no jvm")),
            now,
        );
        m.apply(
            done(BattleStatus::NotRun, &["a 1", "b 1"], &[], Some("stopped")),
            now,
        );
        assert_eq!(
            (m.totals.robot_failed, m.totals.errors, m.totals.not_run),
            (1, 1, 1)
        );
        assert_eq!(m.results.len(), 2, "the stopped battle has no line");
        assert!(m.results[0].contains("robot failed (load)"));
        // The log keeps the failures, the error in red.
        assert!(
            m.log
                .iter()
                .any(|l| l.error && l.text.contains("could not run"))
        );
    }

    #[test]
    fn waiting_has_a_deadline_and_ends_with_the_next_event() {
        let now = t0();
        let mut m = Model::new(now);
        m.apply(
            Event::Waiting {
                seconds: 90,
                why: "no work".into(),
            },
            now,
        );
        let (until, why) = m.waiting.clone().unwrap();
        assert_eq!(until, now + Duration::from_secs(90));
        assert_eq!(why, "no work");
        m.apply(started(0), now);
        assert_eq!(m.waiting, None);
        assert_eq!(countdown(95), "1:35");
    }

    #[test]
    fn the_log_and_the_results_are_bounded() {
        let now = t0();
        let mut m = Model::new(now);
        for i in 0..(LOG_LINES + 50) {
            m.apply(
                Event::Error {
                    text: format!("e{i}"),
                },
                now,
            );
        }
        assert_eq!(m.log.len(), LOG_LINES);
        assert!(
            m.log
                .back()
                .unwrap()
                .text
                .ends_with(&format!("e{}", LOG_LINES + 49))
        );
        for _ in 0..RESULT_LINES + 20 {
            m.apply(done(BattleStatus::Ok, &["a", "b"], &[1.0, 1.0], None), now);
        }
        assert_eq!(m.results.len(), RESULT_LINES);
    }

    #[test]
    fn stopping_is_gentle_first_and_abandons_second() {
        let now = t0();
        let stop = Stop::new();
        let mut m = Model::new(now);
        assert!(!m.request_stop(&stop, now), "nothing runs");
        m.begin(now);
        m.started(
            RunInfo {
                login: "me".into(),
                tier: Tier::New,
                slots: 2,
                divisions: vec![Division::OneVsOne],
                platform: "JVM".into(),
                sandbox: "os".into(),
            },
            now,
        );
        assert_eq!(m.phase, Phase::Running);
        assert!(m.request_stop(&stop, now));
        assert_eq!(m.phase, Phase::Stopping);
        assert!(
            stop.requested() && !stop.aborting(),
            "the first stop is gentle"
        );
        assert!(m.request_stop(&stop, now));
        assert!(stop.aborting(), "the second abandons the battles");
        assert!(!m.request_stop(&stop, now), "and a third does nothing more");
        m.finished(&Summary::default(), now);
        assert_eq!(m.phase, Phase::Idle);
        assert!(m.battles.is_empty());
    }

    #[test]
    fn a_stop_during_the_start_checks_stays_a_stop() {
        let now = t0();
        let stop = Stop::new();
        let mut m = Model::new(now);
        m.begin(now);
        m.request_stop(&stop, now);
        m.started(
            RunInfo {
                login: "me".into(),
                tier: Tier::New,
                slots: 1,
                divisions: vec![Division::OneVsOne],
                platform: "JVM".into(),
                sandbox: "os".into(),
            },
            now,
        );
        assert_eq!(m.phase, Phase::Stopping);
    }

    #[test]
    fn fatal_errors_become_problems_the_page_explains() {
        assert_eq!(Problem::of(&Fatal::LoginNeeded), Problem::LoginNeeded);
        let update = Problem::of(&Fatal::Engine("engine 7, this has 6".into()));
        assert_eq!(update.title(), "Update robocode-s");
        assert!(matches!(
            Problem::of(&Fatal::ClientOutdated("too old".into())),
            Problem::Update(_)
        ));
        assert!(matches!(
            Problem::of(&Fatal::Forbidden("banned".into())),
            Problem::Refused(_)
        ));
        let now = t0();
        let mut m = Model::new(now);
        m.begin(now);
        m.start_failed(&Fatal::LoginNeeded, now);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.problem, Some(Problem::LoginNeeded));
        // A new session clears it; one that ended with a fatal error sets it.
        m.begin(now);
        assert_eq!(m.problem, None);
        m.finished(
            &Summary {
                fatal: Some(Fatal::Sandbox("no sandbox".into())),
                ..Summary::default()
            },
            now,
        );
        assert_eq!(m.problem, Some(Problem::Failed("no sandbox".into())));
    }

    #[test]
    fn melee_needs_five_slots_and_the_page_says_so() {
        use Division::{Melee, OneVsOne};
        assert_eq!(divisions_note(&[OneVsOne], 1, 8), None);
        assert_eq!(divisions_note(&[Melee, OneVsOne], 5, 8), None);
        let note = divisions_note(&[Melee, OneVsOne], 2, 8).unwrap();
        assert!(
            note.contains("Raise") && note.contains("Only 1v1"),
            "{note}"
        );
        let note = divisions_note(&[Melee], 2, 4).unwrap();
        assert!(note.contains("4 cores"), "{note}");
        assert!(divisions_note(&[], 2, 4).is_some());
    }

    #[test]
    fn the_chosen_slots_default_to_the_clients() {
        let mut r = RumbleSettings::default();
        assert_eq!(chosen_slots(&r), default_slots());
        r.parallel = Some(6);
        assert_eq!(chosen_slots(&r), 6);
        r.parallel = Some(0);
        assert_eq!(chosen_slots(&r), 1);
    }

    #[test]
    fn messages_move_the_account_and_ignore_a_cancelled_login() {
        let mut page = Rumble::new();
        let now = t0();
        page.account = Account::SigningIn {
            id: 3,
            cancel: Arc::new(AtomicBool::new(false)),
            code: None,
        };
        // An answer of an older login is ignored.
        page.handle(Msg::LoggedIn(2, Err("old".into())), now);
        assert!(matches!(page.account, Account::SigningIn { .. }));
        page.handle(Msg::LoggedIn(3, Err("denied".into())), now);
        assert!(matches!(&page.account, Account::SignedOut { note: Some(n) } if n == "denied"));
        // Once cancelled (signed out), a late answer changes nothing.
        page.handle(Msg::LoggedIn(3, Err("late".into())), now);
        assert!(matches!(&page.account, Account::SignedOut { note: Some(n) } if n == "denied"));
        // A token the server doesn't know signs out.
        page.account = Account::SignedIn {
            login: "x".into(),
            me: None,
            from_env: false,
            warning: None,
        };
        page.handle(Msg::Me(Err(http::Error::Unauthorized("no".into()))), now);
        assert!(matches!(page.account, Account::SignedOut { .. }));
    }

    #[test]
    fn a_network_error_asking_who_you_are_says_it_tries_again_and_clears() {
        let mut page = Rumble::new();
        let now = t0();
        page.account = Account::SignedIn {
            login: "x".into(),
            me: None,
            from_env: false,
            warning: None,
        };
        let e = http::Error::Network("No such host is known.".into());
        page.handle(Msg::MeAgain(e, Duration::from_secs(5)), now);
        assert!(
            matches!(&page.account, Account::SignedIn { warning: Some(w), .. }
            if w.contains("No such host") && w.contains("Trying again in 5 s"))
        );
        let Some(Rumble {
            account: Account::SignedIn { me: Some(me), .. },
            ..
        }) = Rumble::demo("signed-in")
        else {
            panic!("the signed-in demo has a me");
        };
        page.handle(Msg::Me(Ok(me)), now);
        assert!(matches!(
            &page.account,
            Account::SignedIn {
                warning: None,
                me: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_session_that_needs_a_login_signs_the_page_out() {
        let mut page = Rumble::new();
        let now = t0();
        page.account = Account::SignedIn {
            login: "x".into(),
            me: None,
            from_env: false,
            warning: None,
        };
        page.stop = Some(Stop::new());
        page.model.begin(now);
        page.handle(Msg::StartFailed(Fatal::LoginNeeded), now);
        assert!(!page.active());
        assert!(matches!(page.account, Account::SignedOut { .. }));
        assert_eq!(page.model.problem, Some(Problem::LoginNeeded));
    }

    #[test]
    fn the_demo_states_exist() {
        for state in [
            "out",
            "code",
            "signed-in",
            "contributing",
            "waiting",
            "update",
        ] {
            assert!(Rumble::demo(state).is_some(), "{state}");
        }
        assert!(Rumble::demo("nonsense").is_none());
    }

    #[test]
    fn only_web_addresses_open() {
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
    }
}
