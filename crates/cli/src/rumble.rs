// SPDX-License-Identifier: EPL-2.0

//! `robocode-s rumble`: contribute battles to the rumble, and the owner's admin commands
//! (docs/rumble-backend.md §13.6, §16).

use robocode_s_engine::ENGINE_VERSION;
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::Environment;
use rumble_client::battle::{DEFAULT_MEMORY_MB, Options as BattleOptions, check_engine};
use rumble_client::contributor::{
    Config, Contributor, Event, Stop, Summary, default_slots, release_at_least,
};
use rumble_client::http::Api;
use rumble_client::import::{self, HttpFetcher};
use rumble_client::login::{DeviceCode, DeviceFlow, GITHUB};
use rumble_client::tokenfile::{self, Credentials, TokenFile};
use rumble_core::api::{AlertsResponse, BattleStatus, UserUpdateRequest};
use rumble_core::ids::Division;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where the rumble is, unless `--server` or `$ROBOCODES_RUMBLE_SERVER` say otherwise.
pub const DEFAULT_SERVER: &str = "https://rumble.planethofstede.be";
const SERVER_VAR: &str = "ROBOCODES_RUMBLE_SERVER";
/// Where GitHub is (tests point it at a fake).
const GITHUB_VAR: &str = "ROBOCODES_RUMBLE_GITHUB";

pub const USAGE: &str = "\
usage: robocode-s rumble <command> [options]

The rumble is a continuously running league of robots; volunteers run its battles
(https://rumble.planethofstede.be). You need a GitHub account.

Commands:
  login                  sign in with GitHub (a code to confirm in your browser) and
                         keep an upload token in your settings folder
  logout                 revoke the token and remove it
  status                 who you are, your tier and counters, the server, and whether
                         this robocode-s fits it
  run                    fetch batches of battles, run them, post the results and
                         upload the recordings the rumble asks for; Ctrl+C stops after
                         the running battles (twice: abandons them)
  admin ...              the owner's commands, below

Options of every command:
  --server URL           the rumble ($ROBOCODES_RUMBLE_SERVER; default
                         https://rumble.planethofstede.be; a dev stage is
                         https://dxxxx.cloudfront.net). `login` stores it with the
                         token, the other commands use that one unless it is given.

Options of `run`:
  --parallel N           battles at once (default: half the cores, and one per 2 GiB
                         of memory); a melee battle takes 5 of them
  --divisions LIST       1v1, melee or 1v1,melee (default both; melee needs 5
                         parallel battles)
  --sandbox KIND         what robots run in: os, bwrap, docker, podman, none, as for
                         `robocode-s battle --sandbox` (default os where there is one)
  --unsandboxed          run robots as plain processes (--sandbox none): the rumble
                         runs robots from anyone, so without this it refuses to run
                         without a sandbox (on macOS: Docker or Podman, or this)
  --memory MB            heap per robot JVM (default 512)
  --batches N            stop after N batches have been posted (default: go on)
  --json                 print the events as JSON lines instead of one line per
                         battle and per post
  --data DIR             data directory: JRE, jar cache, pending batches, log
                         ($ROBOCODES_DATA)
  --java PATH            Java executable ($ROBOCODES_JAVA; default: pinned JRE)
  --host-lib DIR         robot host lib directory ($ROBOCODES_HOST_LIB)
  --agent-jar PATH       determinism agent ($ROBOCODES_AGENT_JAR)

  The upload token is in the settings folder's rumble.json, or in
  $ROBOCODES_RUMBLE_TOKEN. Results that could not be posted wait in <data>/rumble/pending
  and are posted at the next start; <data>/rumble/log.txt keeps the last 1 MiB of log.
  `run` also notes its start and end, and a crash's message and backtrace, in
  logs/robocode-s.log in the user data folder (as the GUI does; $ROBOCODES_LOG).

Admin commands (an admin account; they print the server's answer):
  admin import --division 1v1|melee [--list URL|FILE] [--local DIR]
               [--mirror TEMPLATE|none] [--dry-run] [--concurrency N] [--report FILE]
                         seed a division from the RoboWiki participants list (default:
                         the page itself): jars come from --local (files
                         <id>_<version>.jar), the list's URL, then the mirror
                         (default https://robocode-archive.strangeautomata.com/robots/
                         {id}_{version}.jar); --dry-run only downloads and validates;
                         --concurrency downloads at once (default 4); the report is
                         JSON (default import-<division>-<date>.json)
  admin user UID [--rep N] [--state new|quar|ban] [--admin true|false]
               [--trusted true|false]
                         change a user (UID as g<GitHub id>)
  admin bot claim <id> <github login>
                         give a bot to the GitHub user who claimed it
  admin bot remove <id> --reason TEXT
                         take a bot down
  admin alerts           the open alerts (verification mismatches and the like)
  admin config get       print the configuration overrides
  admin config set FILE  replace them with the JSON in FILE
";

/// The parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// `--server`, if given.
    pub server: Option<String>,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Login,
    Logout,
    Status,
    Run(Box<RunOptions>),
    Import(ImportOptions),
    User(UserUpdateRequest),
    BotClaim { bot: String, github: String },
    BotRemove { bot: String, reason: String },
    Alerts,
    ConfigGet,
    ConfigSet(PathBuf),
}

/// Options of `rumble run`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOptions {
    pub parallel: Option<usize>,
    pub divisions: Vec<Division>,
    /// Java, robot host, data directory and sandbox.
    pub env: Environment,
    pub memory_mb: u32,
    pub unsandboxed: bool,
    pub batches: Option<u32>,
    pub json: bool,
}

/// Options of `rumble admin import`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportOptions {
    pub division: Division,
    pub list: Option<String>,
    pub local: Option<PathBuf>,
    /// `None`: the default mirror; `Some(None)`: no mirror (`--mirror none`).
    pub mirror: Option<Option<String>>,
    pub dry_run: bool,
    pub concurrency: usize,
    pub report: Option<PathBuf>,
}

fn number<T: std::str::FromStr>(name: &str, v: String) -> Result<T, String> {
    v.parse()
        .map_err(|_| format!("{name}: not a valid number: {v}"))
}

fn boolean(name: &str, v: String) -> Result<bool, String> {
    match v.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{name}: true or false, not {v}")),
    }
}

fn no_more(args: &[String], command: &str) -> Result<(), String> {
    match args.first() {
        None => Ok(()),
        Some(a) if a.starts_with("--") => Err(format!("{command}: unknown option {a}")),
        Some(a) => Err(format!("{command}: unexpected argument {a}")),
    }
}

impl Parsed {
    pub fn parse(args: &[String]) -> Result<Parsed, String> {
        // --server goes with every command, anywhere.
        let mut server = None;
        let mut rest: Vec<String> = Vec::new();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == "--server" {
                let url = it.next().ok_or("--server needs a value")?;
                server = Some(clean_server(url)?);
            } else {
                rest.push(arg.clone());
            }
        }
        let command = match rest.first().map(String::as_str) {
            None => return Err("no command given".into()),
            Some("login") => {
                no_more(&rest[1..], "login")?;
                Command::Login
            }
            Some("logout") => {
                no_more(&rest[1..], "logout")?;
                Command::Logout
            }
            Some("status") => {
                no_more(&rest[1..], "status")?;
                Command::Status
            }
            Some("run") => Command::Run(Box::new(RunOptions::parse(&rest[1..])?)),
            Some("admin") => parse_admin(&rest[1..])?,
            Some(other) => return Err(format!("unknown command {other}")),
        };
        Ok(Parsed { server, command })
    }
}

/// `https://host`, without a trailing slash.
fn clean_server(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(url.to_string())
    } else {
        Err(format!("--server: not a URL: {url}"))
    }
}

impl RunOptions {
    fn parse(args: &[String]) -> Result<RunOptions, String> {
        let mut o = RunOptions {
            parallel: None,
            divisions: Division::ALL.to_vec(),
            env: Environment::from_env(),
            memory_mb: DEFAULT_MEMORY_MB,
            unsandboxed: false,
            batches: None,
            json: false,
        };
        let mut sandbox_given = false;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            match arg.as_str() {
                "--parallel" => {
                    let n: usize = number(arg, value()?)?;
                    if n < 1 {
                        return Err("--parallel must be at least 1".into());
                    }
                    o.parallel = Some(n);
                }
                "--divisions" => {
                    let list = value()?;
                    let mut divisions = Vec::new();
                    for d in list.split(',').map(str::trim).filter(|d| !d.is_empty()) {
                        let d: Division = d.parse().map_err(|e| format!("--divisions: {e}"))?;
                        if !divisions.contains(&d) {
                            divisions.push(d);
                        }
                    }
                    if divisions.is_empty() {
                        return Err("--divisions needs 1v1, melee or both".into());
                    }
                    o.divisions = divisions;
                }
                "--sandbox" => {
                    o.env.sandbox = value()?.parse().map_err(|e| format!("--sandbox: {e}"))?;
                    sandbox_given = true;
                }
                "--unsandboxed" => o.unsandboxed = true,
                "--memory" => o.memory_mb = number(arg, value()?)?,
                "--batches" => {
                    let n: u32 = number(arg, value()?)?;
                    if n < 1 {
                        return Err("--batches must be at least 1".into());
                    }
                    o.batches = Some(n);
                }
                "--json" => o.json = true,
                "--data" => o.env.data = value()?.into(),
                "--java" => o.env.java = Some(value()?.into()),
                "--host-lib" => o.env.host_lib = value()?.into(),
                "--agent-jar" => o.env.agent_jar = value()?.into(),
                a if a.starts_with("--") => return Err(format!("run: unknown option {a}")),
                a => return Err(format!("run: unexpected argument {a}")),
            }
        }
        if o.unsandboxed {
            // `--unsandboxed` alone is robots as plain processes; with a sandbox it's a
            // contradiction.
            if sandbox_given && o.env.sandbox != Sandbox::None {
                return Err("--unsandboxed contradicts --sandbox".into());
            }
            o.env.sandbox = Sandbox::None;
        }
        Ok(o)
    }
}

fn parse_admin(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None => Err("admin: import, user, bot, alerts or config?".into()),
        Some("import") => ImportOptions::parse(&args[1..]).map(Command::Import),
        Some("user") => parse_user(&args[1..]),
        Some("bot") => parse_bot(&args[1..]),
        Some("alerts") => {
            no_more(&args[1..], "admin alerts")?;
            Ok(Command::Alerts)
        }
        Some("config") => match args.get(1).map(String::as_str) {
            Some("get") => {
                no_more(&args[2..], "admin config get")?;
                Ok(Command::ConfigGet)
            }
            Some("set") => match &args[2..] {
                [file] if !file.starts_with("--") => Ok(Command::ConfigSet(file.into())),
                _ => Err("admin config set takes one file".into()),
            },
            _ => Err("admin config: get or set FILE?".into()),
        },
        Some(other) => Err(format!("admin: unknown command {other}")),
    }
}

impl ImportOptions {
    fn parse(args: &[String]) -> Result<ImportOptions, String> {
        let mut division = None;
        let mut o = ImportOptions {
            division: Division::OneVsOne,
            list: None,
            local: None,
            mirror: None,
            dry_run: false,
            concurrency: 4,
            report: None,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            match arg.as_str() {
                "--division" => {
                    division = Some(
                        value()?
                            .parse::<Division>()
                            .map_err(|e| format!("--division: {e}"))?,
                    )
                }
                "--list" => o.list = Some(value()?),
                "--local" => o.local = Some(value()?.into()),
                "--mirror" => {
                    let m = value()?;
                    o.mirror = Some((m != "none").then_some(m));
                }
                "--dry-run" => o.dry_run = true,
                "--concurrency" => {
                    let n: usize = number(arg, value()?)?;
                    if n < 1 {
                        return Err("--concurrency must be at least 1".into());
                    }
                    o.concurrency = n;
                }
                "--report" => o.report = Some(value()?.into()),
                a if a.starts_with("--") => {
                    return Err(format!("admin import: unknown option {a}"));
                }
                a => return Err(format!("admin import: unexpected argument {a}")),
            }
        }
        o.division = division.ok_or("admin import needs --division 1v1 or --division melee")?;
        Ok(o)
    }
}

fn parse_user(args: &[String]) -> Result<Command, String> {
    let mut request = UserUpdateRequest::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--rep" => request.rep = Some(number(arg, value()?)?),
            "--state" => {
                let state = value()?;
                if !matches!(state.as_str(), "new" | "quar" | "ban") {
                    return Err(format!("--state: new, quar or ban, not {state}"));
                }
                request.state = Some(state);
            }
            "--admin" => request.admin = Some(boolean(arg, value()?)?),
            "--trusted" => request.trusted = Some(boolean(arg, value()?)?),
            a if a.starts_with("--") => return Err(format!("admin user: unknown option {a}")),
            uid if request.uid.is_empty() => request.uid = uid.to_string(),
            a => return Err(format!("admin user: unexpected argument {a}")),
        }
    }
    if request.uid.is_empty() {
        return Err("admin user needs a user id (g<GitHub id>)".into());
    }
    if request.rep.is_none()
        && request.state.is_none()
        && request.admin.is_none()
        && request.trusted.is_none()
    {
        return Err("admin user: nothing to change (--rep, --state, --admin, --trusted)".into());
    }
    Ok(Command::User(request))
}

fn parse_bot(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        Some("claim") => match &args[1..] {
            [bot, github] if !bot.starts_with("--") && !github.starts_with("--") => {
                Ok(Command::BotClaim {
                    bot: bot.clone(),
                    github: github.clone(),
                })
            }
            _ => Err("admin bot claim <id> <github login>".into()),
        },
        Some("remove") => {
            let mut bot = None;
            let mut reason = None;
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--reason" => {
                        reason = Some(rest.next().ok_or("--reason needs a value")?.clone())
                    }
                    a if a.starts_with("--") => {
                        return Err(format!("admin bot remove: unknown option {a}"));
                    }
                    a if bot.is_none() => bot = Some(a.to_string()),
                    a => return Err(format!("admin bot remove: unexpected argument {a}")),
                }
            }
            match (bot, reason) {
                (Some(bot), Some(reason)) if !reason.trim().is_empty() => {
                    Ok(Command::BotRemove { bot, reason })
                }
                _ => Err("admin bot remove <id> --reason TEXT".into()),
            }
        }
        _ => Err("admin bot: claim <id> <github login>, or remove <id> --reason TEXT?".into()),
    }
}

// ---- running a command ----

/// Runs `args` (after `rumble`).
pub fn main(args: &[String]) -> Result<(), String> {
    let parsed = Parsed::parse(args).map_err(|e| format!("{e}\n\n{USAGE}"))?;
    match &parsed.command {
        Command::Login => login(&parsed),
        Command::Logout => logout(&parsed),
        Command::Status => status(&parsed),
        Command::Run(o) => run(&parsed, o),
        Command::Import(o) => run_import(&parsed, o),
        Command::User(request) => {
            let (api, _) = signed_in(&parsed)?;
            let body = serde_json::to_value(request).map_err(|e| e.to_string())?;
            print_json(api.admin_users_update(&body))
        }
        Command::BotClaim { bot, github } => {
            let (api, _) = signed_in(&parsed)?;
            print_json(api.admin_claim(bot, github))
        }
        Command::BotRemove { bot, reason } => {
            let (api, _) = signed_in(&parsed)?;
            print_json(api.admin_remove(bot, reason))
        }
        Command::Alerts => alerts(&parsed),
        Command::ConfigGet => {
            let (api, _) = signed_in(&parsed)?;
            print_json(api.admin_config())
        }
        Command::ConfigSet(file) => {
            let text =
                std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let config: Value =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
            let (api, _) = signed_in(&parsed)?;
            print_json(api.admin_set_config(&config))
        }
    }
}

fn api_for(server: &str, token: Option<String>) -> Api {
    Api::new(&format!("{server}/api/v1"), token, ENGINE_VERSION)
}

/// `--server`, else `$ROBOCODES_RUMBLE_SERVER`, else `stored` (the token file's), else the
/// default.
fn server_for(parsed: &Parsed, stored: Option<&str>) -> Result<String, String> {
    if let Some(s) = &parsed.server {
        return Ok(s.clone());
    }
    if let Some(s) = std::env::var(SERVER_VAR)
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return clean_server(&s).map_err(|e| format!("${SERVER_VAR}: {e}"));
    }
    Ok(stored.map_or_else(|| DEFAULT_SERVER.to_string(), str::to_string))
}

fn credentials() -> Result<Credentials, String> {
    tokenfile::resolve()
        .map_err(|e| format!("reading the token: {e}"))?
        .ok_or_else(|| "not signed in: run `robocode-s rumble login`".to_string())
}

/// The API client with the stored token.
fn signed_in(parsed: &Parsed) -> Result<(Api, Credentials), String> {
    let creds = credentials()?;
    let server = server_for(parsed, creds.server.as_deref())?;
    Ok((api_for(&server, Some(creds.token.clone())), creds))
}

fn print_json(result: Result<Value, rumble_client::http::Error>) -> Result<(), String> {
    let value = result.map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    );
    Ok(())
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

fn login(parsed: &Parsed) -> Result<(), String> {
    let path = tokenfile::path().ok_or(
        "no settings folder to keep the token in (set $ROBOCODES_RUMBLE_CONFIG to a file)",
    )?;
    let server = server_for(parsed, None)?;
    let api = api_for(&server, None);
    let mut flow = DeviceFlow {
        github: std::env::var(GITHUB_VAR)
            .ok()
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| GITHUB.to_string()),
        ..DeviceFlow::default()
    };
    let label = machine_name();
    let mut show = |code: &DeviceCode| {
        println!(
            "To sign in, open {} and enter the code {}",
            code.verification_uri, code.user_code
        );
        println!("Waiting for you to confirm it (Ctrl+C cancels)...");
    };
    let answer = flow
        .login(&api, &label, &mut show)
        .map_err(|e| e.to_string())?;
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
    println!(
        "Signed in to {server} as {} (token for \"{label}\" kept in {})",
        answer.user.login,
        path.display()
    );
    Ok(())
}

fn logout(parsed: &Parsed) -> Result<(), String> {
    let path = tokenfile::path();
    let file = match &path {
        Some(p) => tokenfile::read(p).map_err(|e| e.to_string())?,
        None => None,
    };
    let Some(file) = file else {
        if std::env::var_os(tokenfile::TOKEN_VAR).is_some_and(|t| !t.is_empty()) {
            println!(
                "The token comes from ${}: unset it; nothing is stored",
                tokenfile::TOKEN_VAR
            );
        } else {
            println!("Not signed in");
        }
        return Ok(());
    };
    let server = server_for(parsed, Some(&file.server))?;
    let api = api_for(&server, Some(file.token.clone()));
    match api.revoke_token("current") {
        Ok(_) => println!("The token of {} is revoked", file.login),
        Err(e) => eprintln!(
            "robocode-s rumble: could not revoke the token on {server} ({e}); revoke it on \
             the account page"
        ),
    }
    if let Some(path) = path {
        tokenfile::remove(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("Removed {}", path.display());
    }
    Ok(())
}

fn status(parsed: &Parsed) -> Result<(), String> {
    let creds = tokenfile::resolve().map_err(|e| format!("reading the token: {e}"))?;
    let server = server_for(parsed, creds.as_ref().and_then(|c| c.server.as_deref()))?;
    let api = api_for(&server, creds.as_ref().map(|c| c.token.clone()));
    let info = api
        .info()
        .map_err(|e| format!("can't ask the rumble at {server}: {e}"))?;
    println!(
        "server   {server} (API {}, engine version {}{})",
        info.api,
        info.engine_version,
        if info.paused { ", paused" } else { "" }
    );
    let release = env!("CARGO_PKG_VERSION");
    let mut problems = Vec::new();
    if info.api != 1 {
        problems.push(format!("the server speaks API version {}", info.api));
    }
    if !release_at_least(release, &info.min_client) {
        problems.push(format!(
            "robocode-s {release} is older than the {} the rumble needs",
            info.min_client
        ));
    }
    if let Err(e) = check_engine(&info) {
        problems.push(e);
    }
    if problems.is_empty() {
        println!("client   robocode-s {release}, engine version {ENGINE_VERSION}: compatible");
    } else {
        println!("client   robocode-s {release}, engine version {ENGINE_VERSION}: NOT compatible");
        for p in &problems {
            println!("         {p}");
        }
        println!("         download: {}", info.downloads);
        println!("         or run `robocode-s update`");
    }
    match creds {
        None => println!("account  not signed in: run `robocode-s rumble login`"),
        Some(c) => match api.me() {
            Ok(me) => println!(
                "account  {} ({}), tier {:?}, reputation {}, {} battles, {} verified matches, \
                 {} mismatches{}",
                me.login,
                me.uid,
                me.tier,
                me.reputation,
                me.battles,
                me.verified_matches,
                me.mismatches,
                if me.admin { ", admin" } else { "" }
            ),
            Err(rumble_client::http::Error::Unauthorized(_)) => println!(
                "account  the token {} is not valid (any more): run `robocode-s rumble login`",
                match c.source {
                    tokenfile::Source::Env => "of $ROBOCODES_RUMBLE_TOKEN",
                    tokenfile::Source::File => "in rumble.json",
                }
            ),
            Err(e) => println!("account  could not be read: {e}"),
        },
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err("this robocode-s doesn't fit the rumble".into())
    }
}

fn alerts(parsed: &Parsed) -> Result<(), String> {
    let (api, _) = signed_in(parsed)?;
    let answer: AlertsResponse = api.get_json("admin/alerts").map_err(|e| e.to_string())?;
    if answer.alerts.is_empty() {
        println!("No open alerts");
    }
    for a in &answer.alerts {
        println!("{} {} [{}]: {}", a.id, a.kind, a.reference, a.text);
    }
    Ok(())
}

// ---- run ----

/// One line of the terminal for an event, if it has one: `(to stdout, text)`.
fn describe(event: &Event) -> Option<(bool, String)> {
    match event {
        Event::Batch {
            batch,
            division,
            battles,
            ..
        } => Some((
            false,
            format!("batch {batch}: {battles} {division} battles"),
        )),
        Event::BattleDone {
            division,
            bots,
            status,
            scores,
            ranks,
            duration_ms,
            detail,
            ..
        } => {
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
                BattleStatus::Error => {
                    format!("could not run ({})", detail.as_deref().unwrap_or("?"))
                }
                BattleStatus::NotRun => match detail.as_deref() {
                    Some("stopped") | None => return None,
                    Some(d) => format!("not run ({d})"),
                },
            };
            Some((
                true,
                format!("{division} {who}: {outcome}{}", seconds.unwrap_or_default()),
            ))
        }
        Event::Posted {
            batch,
            accepted,
            rejected,
            duplicate,
            battles,
            reputation,
            tier,
        } => Some((
            true,
            format!(
                "posted {batch}: {accepted} accepted, {rejected} rejected{} (you: {battles} \
                 battles, reputation {reputation}, {tier:?})",
                if *duplicate { ", seen before" } else { "" }
            ),
        )),
        Event::Uploaded { n, .. } => Some((true, format!("uploaded {n} recordings"))),
        Event::Waiting { seconds, why } => Some((false, format!("waiting {seconds} s: {why}"))),
        Event::Error { text } => Some((false, format!("error: {text}"))),
        Event::BattleStarted { .. } | Event::Round { .. } => None,
    }
}

fn summary_line(s: &Summary) -> String {
    format!(
        "{} battles run ({} robot failures, {} not run, {} errors); {} batches posted, {} \
         recordings uploaded",
        s.battles_ok, s.robot_failed, s.not_run, s.errors, s.batches_posted, s.uploaded
    )
}

fn run(parsed: &Parsed, o: &RunOptions) -> Result<(), String> {
    // A long unattended run notes its start and end, and a panic, in the log file
    // (`<data>/rumble/log.txt` keeps the session's lines).
    let log = rumble_client::log::init("robocode-s");
    let result = run_logged(parsed, o, log.as_deref());
    match &result {
        Ok(()) => rumble_client::log::exit("0"),
        Err(e) => rumble_client::log::exit(&format!("failed: {e}")),
    }
    result
}

fn run_logged(parsed: &Parsed, o: &RunOptions, log: Option<&Path>) -> Result<(), String> {
    let creds = credentials()?;
    let server = server_for(parsed, creds.server.as_deref())?;
    let slots = o.parallel.unwrap_or_else(default_slots);
    let mut options = BattleOptions::new(o.env.clone());
    options.memory_mb = o.memory_mb;
    options.allow_unsandboxed = o.unsandboxed;
    options.slots = slots;
    let mut config = Config::new(&server, &creds.token, options);
    config.divisions = o.divisions.clone();
    config.max_batches = o.batches;
    if let Some(log) = log {
        eprintln!("robocode-s rumble: logging to {}", log.display());
    }
    eprintln!("robocode-s rumble: checking {server}...");
    let contributor = Contributor::new(config).map_err(|e| e.to_string())?;
    let divisions: Vec<String> = contributor
        .divisions()
        .iter()
        .map(Division::to_string)
        .collect();
    eprintln!(
        "robocode-s rumble: {} ({:?}) contributes {} on {} parallel battles; robots run on {} \
         in sandbox {}",
        contributor.me().login,
        contributor.me().tier,
        divisions.join(" and "),
        contributor.slots(),
        contributor.platform(),
        o.env.sandbox
    );
    let stop = Stop::new();
    let handle = stop.clone();
    let _ = ctrlc::set_handler(move || {
        handle.stop();
        if handle.aborting() {
            eprintln!("robocode-s rumble: abandoning the running battles");
        } else {
            eprintln!(
                "robocode-s rumble: stopping after the running battles (Ctrl+C again abandons \
                 them)"
            );
        }
    });
    let json = o.json;
    let summary = contributor.run(&stop, |event| {
        if json {
            if let Ok(line) = serde_json::to_string(&event) {
                println!("{line}");
            }
        } else if let Some((out, text)) = describe(&event) {
            if out {
                println!("{text}");
            } else {
                eprintln!("{text}");
            }
        }
    });
    eprintln!("robocode-s rumble: {}", summary_line(&summary));
    match summary.fatal {
        Some(f) => Err(f.to_string()),
        None => Ok(()),
    }
}

// ---- admin import ----

fn run_import(parsed: &Parsed, o: &ImportOptions) -> Result<(), String> {
    let fetcher = HttpFetcher::new();
    let source = o
        .list
        .clone()
        .unwrap_or_else(|| import::default_list(o.division).to_string());
    let list = import::load_list(&source, &fetcher)?;
    eprintln!(
        "robocode-s rumble: {} entries in {source}{}",
        list.entries.len(),
        if list.duplicates.is_empty() {
            String::new()
        } else {
            format!(", {} duplicates", list.duplicates.len())
        }
    );
    let mut options = import::Options::new(o.division);
    options.local = o.local.clone();
    if let Some(mirror) = &o.mirror {
        options.mirror = mirror.clone();
    }
    options.concurrency = o.concurrency;
    options.dry_run = o.dry_run;
    let api = if o.dry_run {
        None
    } else {
        Some(signed_in(parsed)?.0)
    };
    let progress = |row: &import::EntryReport, done: usize, total: usize| {
        eprintln!(
            "[{done}/{total}] {} {}: {}{}",
            row.id,
            row.version,
            row.result,
            row.error
                .as_ref()
                .map_or(String::new(), |e| format!(" ({e})"))
        );
    };
    let report = import::run(&list, &options, &fetcher, api.as_ref(), &progress);
    let path = o.report.clone().unwrap_or_else(|| {
        PathBuf::from(import::default_report_name(o.division, &import::today()))
    });
    std::fs::write(&path, report.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
    print!("{}", report.summary());
    println!("Report: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn parse(s: &str) -> Result<Parsed, String> {
        Parsed::parse(&args(s))
    }

    fn run_options(s: &str) -> RunOptions {
        match parse(s).unwrap().command {
            Command::Run(o) => *o,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_server_goes_with_any_command() {
        let p = parse("login --server https://d1.cloudfront.net/").unwrap();
        assert_eq!(p.command, Command::Login);
        assert_eq!(p.server.as_deref(), Some("https://d1.cloudfront.net"));
        let p = parse("--server http://127.0.0.1:8080 status").unwrap();
        assert_eq!(p.command, Command::Status);
        assert_eq!(p.server.as_deref(), Some("http://127.0.0.1:8080"));
        let p = parse("admin alerts --server https://x.y").unwrap();
        assert_eq!(p.command, Command::Alerts);
        assert!(parse("status --server").is_err());
        assert!(parse("status --server rumble.example").is_err());
        assert_eq!(parse("logout").unwrap().server, None);
    }

    #[test]
    fn parses_the_run_options() {
        let o = run_options(
            "run --parallel 6 --divisions melee,1v1 --sandbox none --unsandboxed --memory 256 \
             --batches 3 --json --data /tmp/d",
        );
        assert_eq!(o.parallel, Some(6));
        assert_eq!(o.divisions, [Division::Melee, Division::OneVsOne]);
        assert_eq!(o.env.sandbox, Sandbox::None);
        assert!(o.unsandboxed && o.json);
        assert_eq!((o.memory_mb, o.batches), (256, Some(3)));
        assert_eq!(o.env.data, PathBuf::from("/tmp/d"));
        let o = run_options("run");
        assert_eq!(o.parallel, None);
        assert_eq!(o.divisions, Division::ALL);
        assert_eq!((o.memory_mb, o.batches, o.unsandboxed), (512, None, false));
        assert_eq!(
            run_options("run --divisions 1v1").divisions,
            [Division::OneVsOne]
        );
    }

    #[test]
    fn rejects_bad_run_options() {
        assert!(parse("run --parallel 0").is_err());
        assert!(parse("run --parallel x").is_err());
        assert!(parse("run --parallel").is_err());
        assert!(parse("run --batches 0").is_err());
        assert!(parse("run --divisions 2v2").is_err());
        assert!(parse("run --divisions ,").is_err());
        assert!(parse("run --sandbox nothing").is_err());
        assert!(parse("run --bogus").is_err());
        assert!(parse("run now").is_err());
        // Without a sandbox it is --unsandboxed, with one it is not allowed.
        assert!(parse("run --sandbox none").is_ok());
        assert!(parse("run --unsandboxed --sandbox none").is_ok());
        assert!(
            parse("run --unsandboxed --sandbox os").is_err()
                || cfg!(not(any(target_os = "linux", windows)))
        );
        assert_eq!(run_options("run --unsandboxed").env.sandbox, Sandbox::None);
        assert!(parse("login now").is_err());
        assert!(parse("status --json").is_err());
        assert!(parse("").is_err());
        assert!(parse("frobnicate").is_err());
    }

    #[test]
    fn parses_the_import_options() {
        let Command::Import(o) = parse(
            "admin import --division melee --list list.txt --local /corpus --mirror none \
             --dry-run --concurrency 8 --report r.json",
        )
        .unwrap()
        .command
        else {
            panic!()
        };
        assert_eq!(o.division, Division::Melee);
        assert_eq!(o.list.as_deref(), Some("list.txt"));
        assert_eq!(o.local, Some("/corpus".into()));
        assert_eq!(o.mirror, Some(None));
        assert!(o.dry_run);
        assert_eq!((o.concurrency, o.report), (8, Some("r.json".into())));
        let Command::Import(o) =
            parse("admin import --division 1v1 --mirror http://m/{id}_{version}.jar")
                .unwrap()
                .command
        else {
            panic!()
        };
        assert_eq!(o.mirror, Some(Some("http://m/{id}_{version}.jar".into())));
        assert_eq!((o.concurrency, o.dry_run), (4, false));
        assert!(parse("admin import").is_err());
        assert!(parse("admin import --division 2v2").is_err());
        assert!(parse("admin import --division 1v1 --concurrency 0").is_err());
    }

    #[test]
    fn parses_the_admin_commands() {
        let Command::User(u) =
            parse("admin user g583231 --rep 50 --state quar --admin true --trusted false")
                .unwrap()
                .command
        else {
            panic!()
        };
        assert_eq!(u.uid, "g583231");
        assert_eq!(u.rep, Some(50));
        assert_eq!(u.state.as_deref(), Some("quar"));
        assert_eq!((u.admin, u.trusted), (Some(true), Some(false)));
        assert!(parse("admin user").is_err());
        assert!(parse("admin user g1").is_err());
        assert!(parse("admin user g1 --state banned").is_err());
        assert!(parse("admin user g1 --admin yes").is_err());
        assert!(parse("admin user g1 g2 --rep 1").is_err());

        assert_eq!(
            parse("admin bot claim jk.mega.DrussGT nhofsted")
                .unwrap()
                .command,
            Command::BotClaim {
                bot: "jk.mega.DrussGT".into(),
                github: "nhofsted".into()
            }
        );
        assert!(parse("admin bot claim jk.mega.DrussGT").is_err());
        let Command::BotRemove { bot, reason } =
            parse("admin bot remove a.B --reason author-request")
                .unwrap()
                .command
        else {
            panic!()
        };
        assert_eq!((bot.as_str(), reason.as_str()), ("a.B", "author-request"));
        assert!(parse("admin bot remove a.B").is_err());
        assert!(parse("admin bot remove --reason x").is_err());
        assert!(parse("admin bot burn a.B").is_err());

        assert_eq!(parse("admin alerts").unwrap().command, Command::Alerts);
        assert_eq!(
            parse("admin config get").unwrap().command,
            Command::ConfigGet
        );
        assert_eq!(
            parse("admin config set c.json").unwrap().command,
            Command::ConfigSet("c.json".into())
        );
        assert!(parse("admin config set").is_err());
        assert!(parse("admin config").is_err());
        assert!(parse("admin").is_err());
        assert!(parse("admin frobnicate").is_err());
    }

    #[test]
    fn the_help_names_every_command() {
        for word in [
            "login",
            "logout",
            "status",
            "run",
            "--parallel",
            "--divisions",
            "--sandbox",
            "--unsandboxed",
            "--memory",
            "--batches",
            "--json",
            "admin import",
            "admin user",
            "admin bot claim",
            "admin bot remove",
            "admin alerts",
            "admin config get",
            "admin config set",
            "--server",
            "ROBOCODES_RUMBLE_SERVER",
            "--trusted",
        ] {
            assert!(USAGE.contains(word), "{word}");
        }
    }

    fn done(status: BattleStatus, bots: &[&str], scores: &[f64], detail: Option<&str>) -> Event {
        Event::BattleDone {
            batch: "b-1".into(),
            index: 0,
            division: Division::OneVsOne,
            bots: bots.iter().map(|b| b.to_string()).collect(),
            status,
            scores: scores.to_vec(),
            ranks: vec![1, 2],
            duration_ms: Some(41_200),
            detail: detail.map(String::from),
        }
    }

    #[test]
    fn battle_lines_read_like_the_spec() {
        let e = done(
            BattleStatus::Ok,
            &["jk.mega.DrussGT 3.1.16", "voidious.Diamond 1.8.28"],
            &[54.2, 45.8],
            None,
        );
        assert_eq!(
            describe(&e),
            Some((
                true,
                "1v1 jk.mega.DrussGT 3.1.16 vs voidious.Diamond 1.8.28: 54.2% (41 s)".into()
            ))
        );
        let e = done(
            BattleStatus::RobotFailed,
            &["a.B", "c.D"],
            &[],
            Some("c.D Load: boom"),
        );
        assert_eq!(
            describe(&e).unwrap().1,
            "1v1 a.B vs c.D: robot failed (c.D Load: boom) (41 s)"
        );
        assert_eq!(
            describe(&done(
                BattleStatus::NotRun,
                &["a.B", "c.D"],
                &[],
                Some("stopped")
            )),
            None
        );
        let e = Event::BattleDone {
            batch: "b-1".into(),
            index: 0,
            division: Division::Melee,
            bots: (0..10).map(|i| format!("b.B{i}")).collect(),
            status: BattleStatus::Ok,
            scores: vec![],
            ranks: vec![3],
            duration_ms: Some(41_200),
            detail: None,
        };
        assert_eq!(
            describe(&e).unwrap().1,
            "melee b.B0 + 9 others: rank 3 of 10 (41 s)"
        );
        let posted = Event::Posted {
            batch: "b-1".into(),
            accepted: 3,
            rejected: 0,
            duplicate: false,
            battles: 10,
            reputation: 5,
            tier: rumble_core::api::Tier::New,
        };
        assert!(
            describe(&posted)
                .unwrap()
                .1
                .starts_with("posted b-1: 3 accepted, 0 rejected")
        );
    }
}
