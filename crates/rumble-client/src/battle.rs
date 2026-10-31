// SPDX-License-Identifier: EPL-2.0

//! Running one battle of a batch (docs/rumble-backend.md §13.1, §13.2): the robots in a
//! local battle as `robocode-s battle` runs it, deterministic, with no data files from
//! earlier battles, taken from a warm pool, observed turn by turn for the hashes and the
//! tracks (§13.4). The outcome is the battle's entry of the results post.

use crate::hash::{self, Digest32, RoundHasher};
use crate::track::{self, Encoded};
use robocode_s_engine::{BattleRules, ENGINE_VERSION};
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::Sandbox;
use robocode_s_runner::local::{Environment, host_info, prepare_with, run_local_battle};
use robocode_s_runner::pool::Pool;
use robocode_s_server::{Connection, DETERMINISTIC_RULES, ServerConfig};
use rumble_core::api::{
    BattleResult, BattleStatus, BotColors, Failure, FailureReason, Info, RobotResult, RoundResult,
    Rules, TrackClaim, WorkBattle, WorkBot,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Instant;

/// The robots' heap (`--memory`).
pub const DEFAULT_MEMORY_MB: u32 = 512;

/// The longest failure detail sent (the end of a robot's error output).
const MAX_DETAIL: usize = 500;

/// Refuses to run the rumble's battles with an engine or deterministic rules other than the
/// server's (§13.1): results of another engine are never counted.
pub fn check_engine(info: &Info) -> Result<(), String> {
    if info.engine_version != ENGINE_VERSION {
        return Err(format!(
            "the rumble ranks battles of engine version {}, this robocode-s has engine \
             version {ENGINE_VERSION}: update robocode-s (or wait for the rumble to move to it)",
            info.engine_version
        ));
    }
    if info.deterministic_rules != DETERMINISTIC_RULES {
        return Err(format!(
            "the rumble uses deterministic rules version {}, this robocode-s follows version \
             {DETERMINISTIC_RULES}: update robocode-s",
            info.deterministic_rules
        ));
    }
    Ok(())
}

/// The rumble runs other people's jars: no sandbox only when explicitly allowed
/// (`--unsandboxed`, §13.1), and the chosen one must be there.
pub fn check_sandbox(sandbox: Sandbox, allow_unsandboxed: bool) -> Result<(), String> {
    if sandbox == Sandbox::None && !allow_unsandboxed {
        return Err(
            "the rumble runs robots from anyone: it needs a sandbox (--sandbox os, bwrap, \
             docker or podman); to run them as plain processes anyway, give --unsandboxed"
                .into(),
        );
    }
    if let Some(why) = sandbox.unavailable() {
        return Err(format!("sandbox {sandbox}: {why}"));
    }
    Ok(())
}

/// How battles are run.
#[derive(Debug, Clone)]
pub struct Options {
    /// Java, the robot host, the determinism agent, the data folder and the sandbox; its
    /// `jars` and `fresh_data` are each battle's own.
    pub env: Environment,
    pub memory_mb: u32,
    /// `--unsandboxed`: allows [`Sandbox::None`].
    pub allow_unsandboxed: bool,
    /// Parallel battle slots (§13.2); the pool keeps `2 × slots` hosts warm.
    pub slots: usize,
    /// Keep every round's snapshots in [`Outcome::snapshots`] (tests, diagnosis).
    pub keep_snapshots: bool,
}

impl Options {
    pub fn new(env: Environment) -> Self {
        Self {
            env,
            memory_mb: DEFAULT_MEMORY_MB,
            allow_unsandboxed: false,
            slots: 1,
            keep_snapshots: false,
        }
    }
}

/// What a battle came to: its entry of the results post, and its rounds' encoded tracks
/// (for the uploads the server asks for), by round. `tracks` is empty unless `ok`.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub result: BattleResult,
    pub tracks: Vec<Vec<u8>>,
    /// Why the battle is `error` (or `notRun`), for the log: the API carries no reason.
    pub why: Option<String>,
    /// Per round, the snapshots the observer saw, with [`Options::keep_snapshots`].
    pub snapshots: Vec<Vec<v1::TurnSnapshot>>,
}

impl Outcome {
    fn status(index: u32, status: BattleStatus) -> Self {
        Self {
            result: BattleResult {
                index,
                status,
                duration_ms: None,
                battle_hash: None,
                robots: None,
                rounds: None,
                failure: None,
            },
            tracks: Vec::new(),
            why: None,
            snapshots: Vec::new(),
        }
    }

    fn failed(index: u32, robot: usize, reason: FailureReason, detail: &str) -> Self {
        let mut outcome = Self::status(index, BattleStatus::RobotFailed);
        let detail = detail.trim();
        let detail = match detail.char_indices().nth_back(MAX_DETAIL - 1) {
            Some((i, _)) if detail.len() > MAX_DETAIL => &detail[i..],
            _ => detail,
        };
        outcome.result.failure = Some(Failure {
            robot: robot as u32,
            reason,
            detail: (!detail.is_empty()).then(|| detail.to_string()),
        });
        outcome
    }
}

/// Progress of a running battle, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Round `n` (1-based) of `of` is over.
    Round { n: u32, of: u32 },
}

/// The name a bot goes by in the battle: `"<id> <version>"` (just the id without one).
pub fn robot_name(bot: &WorkBot) -> String {
    if bot.version.is_empty() {
        bot.id.clone()
    } else {
        format!("{} {}", bot.id, bot.version)
    }
}

/// The engine's rules of a work item.
pub fn engine_rules(rules: &Rules) -> BattleRules {
    BattleRules {
        width: rules.width as i32,
        height: rules.height as i32,
        num_rounds: rules.rounds as i32,
        gun_cooling_rate: rules.gun_cooling_rate,
        inactivity_time: i64::from(rules.inactivity_time),
        hide_enemy_names: rules.hide_enemy_names,
        sentry_border_size: rules.sentry_border as i32,
    }
}

/// Runs battles; shared by the battle threads (one warm pool for the process, §13.2).
#[derive(Debug)]
pub struct Runner {
    options: Options,
    pool: Arc<Pool>,
}

/// A folder name of its own for each runner in this process (the GUI may have several, tests
/// do), ending in `-<pid>` as `housekeeping::remove_stale_runs` expects.
fn run_folder(kind: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{kind}{n}-{}", std::process::id())
}

impl Runner {
    /// Checks the sandbox policy ([`check_sandbox`]) and sets up the pool.
    pub fn new(options: Options) -> Result<Self, String> {
        check_sandbox(options.env.sandbox, options.allow_unsandboxed)?;
        let dir = options.env.data.join("pool").join(run_folder("rumble"));
        Ok(Self {
            options,
            pool: Arc::new(Pool::new(dir)),
        })
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Checks the robot host's deterministic rules (`runner::local::host_info`, §13.1), as
    /// `robocode-s battle` does: finds Java (downloading the pinned JRE if needed) and asks
    /// the robot host. Returns the platform robots run on (`JVM 21.0.4`).
    pub fn check_host(&self) -> Result<String, String> {
        // Each battle has its own jars: none here.
        let env = Environment {
            jars: Vec::new(),
            ..self.options.env.clone()
        };
        let host = env.host(true)?;
        let home = self
            .options
            .env
            .data
            .join("battles")
            .join(run_folder("host-info"));
        host_info(&host, &home).map(|(platform, _)| platform)
    }

    /// Runs `battle` with its bots' jars (`jars[i]` for `battle.bots[i]`).
    pub fn run(&self, battle: &WorkBattle, jars: &[PathBuf]) -> Outcome {
        self.run_with(battle, jars, None, |_| {})
    }

    /// [`Runner::run`], reporting `progress`; `abort` set stops it (the battle is then
    /// `notRun`).
    pub fn run_with(
        &self,
        battle: &WorkBattle,
        jars: &[PathBuf],
        abort: Option<&AtomicBool>,
        mut progress: impl FnMut(Progress),
    ) -> Outcome {
        let index = battle.index;
        // A data snapshot this client doesn't understand: running without it would misrun.
        if let Some(bot) = battle.bots.iter().find(|b| b.data.is_some()) {
            return not_run(
                index,
                &format!(
                    "{}: a data snapshot, which this client can't use",
                    robot_name(bot)
                ),
            );
        }
        if jars.len() != battle.bots.len() {
            return error(
                index,
                &format!("{} jars for {} bots", jars.len(), battle.bots.len()),
            );
        }
        if let Err(e) = check_sandbox(self.options.env.sandbox, self.options.allow_unsandboxed) {
            return error(index, &e);
        }
        let started = Instant::now();
        let names: Vec<String> = battle.bots.iter().map(robot_name).collect();
        let mut env_jars: Vec<PathBuf> = Vec::new();
        for jar in jars {
            if !env_jars.contains(jar) {
                env_jars.push(jar.clone());
            }
        }
        let env = Environment {
            jars: env_jars,
            fresh_data: true,
            ..self.options.env.clone()
        };
        let config = ServerConfig {
            rules: engine_rules(&battle.rules),
            seed: battle.seed,
            deterministic: true,
            record: None,
            ..Default::default()
        };
        let warm = 2 * self.options.slots.max(1);
        let (host, local) = match prepare_with(
            &env,
            &names,
            config,
            self.options.memory_mb,
            Some((&self.pool, warm)),
        ) {
            Ok(prepared) => prepared,
            Err(e) => {
                return match named_robot(&e, &names) {
                    Some(robot) => Outcome::failed(index, robot, FailureReason::Load, &e),
                    None => error(index, &e),
                };
            }
        };
        if local.robots.len() != names.len() {
            local.remove_dir();
            return error(
                index,
                &format!(
                    "{} robots described for {} bots (a team?)",
                    local.robots.len(),
                    names.len()
                ),
            );
        }

        let rounds = battle.rules.rounds;
        let mut observer = Observer::new(&battle.rules);
        observer.keep_snapshots = self.options.keep_snapshots;
        let (controls_tx, controls) = mpsc::channel();
        let mut aborting = false;
        let ran = run_local_battle(&host, &local, Some(controls), |msg| {
            if let Some(abort) = abort
                && !aborting
                && abort.load(Ordering::Relaxed)
            {
                aborting = true;
                let _ = controls_tx.send(v1::ObserverControl {
                    control: Some(v1::observer_control::Control::Abort(true)),
                });
            }
            if let Some(n) = observer.observe(msg) {
                progress(Progress::Round { n, of: rounds });
            }
        });
        let ran = match ran {
            Ok(ran) => ran,
            Err(e) => return error(index, &e),
        };
        if aborting || ran.outcome.aborted {
            return not_run(index, "stopped");
        }
        for (i, c) in ran.outcome.connections.iter().enumerate() {
            let detail = ran.stderr.get(i).map_or("", String::as_str);
            match c {
                Connection::Kept => {}
                Connection::Never => {
                    let detail = if detail.is_empty() {
                        "never connected"
                    } else {
                        detail
                    };
                    return Outcome::failed(index, i, FailureReason::Load, detail);
                }
                Connection::Lost => {
                    return Outcome::failed(index, i, FailureReason::Crash, detail);
                }
            }
        }
        if observer.error.is_some() || observer.rounds.len() != rounds as usize {
            let why = observer.error.clone().unwrap_or_else(|| {
                format!("{} of {rounds} rounds observed", observer.rounds.len())
            });
            return error(index, &why);
        }
        let results = &ran.outcome.results;
        let pairs: Vec<(Digest32, Digest32)> = observer
            .rounds
            .iter()
            .map(|r| (r.hash, r.track.sha256))
            .collect();
        let battle_hash = hash::battle_hash(ENGINE_VERSION, battle.seed, &pairs, results);
        let robots = results
            .iter()
            .enumerate()
            .map(|(i, r)| RobotResult {
                colors: observer.colors.get(i).cloned().flatten(),
                threads: ran.outcome.threads.get(i).copied().unwrap_or(false),
                hard_limit: ran.outcome.hard_limits.get(i).copied().unwrap_or(false),
                ..robot_result(r)
            })
            .collect();
        let mut tracks = Vec::with_capacity(observer.rounds.len());
        let mut snapshots = Vec::new();
        let rounds = observer
            .rounds
            .into_iter()
            .map(|r| {
                let claim = TrackClaim {
                    sha256: hash::hex(&r.track.sha256),
                    size: r.track.bytes.len() as u64,
                };
                tracks.push(r.track.bytes);
                if self.options.keep_snapshots {
                    snapshots.push(r.snapshots);
                }
                RoundResult {
                    turns: r.turns,
                    winner: match r.firsts[..] {
                        [only] => only as i32,
                        _ => -1,
                    },
                    firsts: r.firsts,
                    scores: r.scores,
                    hash: hash::hex(&r.hash),
                    track: Some(claim),
                }
            })
            .collect();
        Outcome {
            result: BattleResult {
                index,
                status: BattleStatus::Ok,
                duration_ms: Some(started.elapsed().as_millis() as u64),
                battle_hash: Some(hash::hex(&battle_hash)),
                robots: Some(robots),
                rounds: Some(rounds),
                failure: None,
            },
            tracks,
            why: None,
            snapshots,
        }
    }
}

fn error(index: u32, why: &str) -> Outcome {
    Outcome {
        why: Some(why.to_string()),
        ..Outcome::status(index, BattleStatus::Error)
    }
}

fn not_run(index: u32, why: &str) -> Outcome {
    Outcome {
        why: Some(why.to_string()),
        ..Outcome::status(index, BattleStatus::NotRun)
    }
}

/// The robot a failed description names (`robot <name> not found ...`, or else its name).
fn named_robot(error: &str, names: &[String]) -> Option<usize> {
    names
        .iter()
        .position(|n| error.contains(&format!("robot {n} ")))
        .or_else(|| names.iter().position(|n| error.contains(n.as_str())))
}

fn robot_result(r: &v1::Results) -> RobotResult {
    let f = |v: Option<f64>| v.unwrap_or(0.0);
    RobotResult {
        score: f(r.score),
        survival: f(r.survival),
        last_survivor_bonus: f(r.last_survivor_bonus),
        bullet_damage: f(r.bullet_damage),
        bullet_damage_bonus: f(r.bullet_damage_bonus),
        ram_damage: f(r.ram_damage),
        ram_damage_bonus: f(r.ram_damage_bonus),
        firsts: r.firsts.max(0) as u32,
        seconds: r.seconds.max(0) as u32,
        thirds: r.thirds.max(0) as u32,
        rank: r.rank.max(0) as u32,
        colors: None,
        threads: false,
        hard_limit: false,
    }
}

/// The tank colours a robot set itself (`Colors.set_parts`), `None` where it set none of
/// body, gun and radar, or its host doesn't say (older hosts: their battles report none).
pub fn set_colors(colors: Option<&v1::Colors>) -> Option<BotColors> {
    let c = colors?;
    let part = |bit: u32, argb: i32| (c.set_parts & bit != 0).then(|| BotColors::hex(argb));
    (c.set_parts & 0b111 != 0).then(|| BotColors {
        body: part(1, c.body),
        gun: part(2, c.gun),
        radar: part(4, c.radar),
    })
}

/// One finished round, as observed.
pub struct ObservedRound {
    pub turns: u32,
    /// The robots whose `firsts` rose in the round (classic can give two robots a first).
    pub firsts: Vec<u32>,
    pub scores: Vec<f64>,
    pub hash: Digest32,
    pub track: Encoded,
    /// Its snapshots, when kept ([`Observer::keep_snapshots`]).
    pub snapshots: Vec<v1::TurnSnapshot>,
}

/// What the battle's observer makes of its messages: per round its hash, track, turns,
/// winner and scores.
pub struct Observer {
    rules: Rules,
    snapshots: Vec<v1::TurnSnapshot>,
    hasher: Option<RoundHasher>,
    last: Option<v1::RoundResult>,
    /// Keep each round's snapshots in [`ObservedRound::snapshots`] (tests).
    pub keep_snapshots: bool,
    pub rounds: Vec<ObservedRound>,
    /// Something the observer didn't expect (a snapshot of another round).
    pub error: Option<String>,
    /// Per robot, the tank colours it set ([`set_colors`]) in the first snapshot in which
    /// it had set any: most robots set them once, at the start of `run()`.
    pub colors: Vec<Option<BotColors>>,
}

impl Observer {
    pub fn new(rules: &Rules) -> Self {
        Self {
            rules: rules.clone(),
            snapshots: Vec::new(),
            hasher: None,
            last: None,
            keep_snapshots: false,
            rounds: Vec::new(),
            error: None,
            colors: Vec::new(),
        }
    }

    /// Takes one message; returns the number of rounds over when a round ended.
    pub fn observe(&mut self, msg: &server_message::Msg) -> Option<u32> {
        match msg {
            server_message::Msg::TurnSnapshot(s) => {
                let round = self.rounds.len() as u32;
                if s.round != round as i32 || s.turn != self.snapshots.len() as i64 {
                    self.error.get_or_insert_with(|| {
                        format!(
                            "snapshot of round {} turn {} where round {round} turn {} was due",
                            s.round,
                            s.turn,
                            self.snapshots.len()
                        )
                    });
                }
                self.hasher
                    .get_or_insert_with(|| RoundHasher::new(round))
                    .snapshot(s);
                if self.colors.len() < s.robots.len() {
                    self.colors.resize(s.robots.len(), None);
                }
                for (seen, robot) in self.colors.iter_mut().zip(&s.robots) {
                    if seen.is_none() {
                        *seen = set_colors(robot.colors.as_ref());
                    }
                }
                self.snapshots.push(s.clone());
                None
            }
            server_message::Msg::RoundResult(r) => {
                let round = self.rounds.len() as u32;
                let hasher = self
                    .hasher
                    .take()
                    .unwrap_or_else(|| RoundHasher::new(round));
                let snapshots = std::mem::take(&mut self.snapshots);
                let track = Encoded::new(&track::from_snapshots(&self.rules, &snapshots));
                let (scores, firsts) = round_scores(self.last.as_ref(), r);
                self.rounds.push(ObservedRound {
                    turns: snapshots.len().saturating_sub(1) as u32,
                    firsts,
                    scores,
                    hash: hasher.finish(r),
                    track,
                    snapshots: if self.keep_snapshots {
                        snapshots
                    } else {
                        Vec::new()
                    },
                });
                self.last = Some(r.clone());
                Some(self.rounds.len() as u32)
            }
            _ => None,
        }
    }
}

/// Each robot's score in the round (the difference of the cumulative results) and the robots
/// whose `firsts` rose: none, one, or several when robots died together.
fn round_scores(before: Option<&v1::RoundResult>, after: &v1::RoundResult) -> (Vec<f64>, Vec<u32>) {
    let prior = |i: usize| before.and_then(|b| b.results.get(i));
    let scores = after
        .results
        .iter()
        .enumerate()
        .map(|(i, r)| r.score.unwrap_or(0.0) - prior(i).and_then(|p| p.score).unwrap_or(0.0))
        .collect();
    let firsts = after
        .results
        .iter()
        .enumerate()
        .filter(|(i, r)| r.firsts > prior(*i).map_or(0, |p| p.firsts))
        .map(|(i, _)| i as u32)
        .collect();
    (scores, firsts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(scores: &[(f64, i32)]) -> v1::RoundResult {
        v1::RoundResult {
            round: 0,
            results: scores
                .iter()
                .map(|&(score, firsts)| v1::Results {
                    score: Some(score),
                    firsts,
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn round_scores_are_differences_and_the_winner_gained_a_first() {
        let first = results(&[(100.0, 1), (20.0, 0)]);
        assert_eq!(round_scores(None, &first), (vec![100.0, 20.0], vec![0]));
        let second = results(&[(130.0, 1), (140.0, 1)]);
        assert_eq!(
            round_scores(Some(&first), &second),
            (vec![30.0, 120.0], vec![1])
        );
        assert_eq!(round_scores(Some(&second), &second).1, Vec::<u32>::new());
        // Robots that die together each get a first (classic): both are reported.
        let tie = results(&[(150.0, 2), (150.0, 2)]);
        assert_eq!(round_scores(Some(&second), &tie).1, vec![0, 1]);
    }

    #[test]
    fn robots_report_the_colours_they_set_first() {
        let classic = 0xFF29_298C_u32 as i32;
        let colors = |set_parts: u32, body: i32| v1::Colors {
            body,
            gun: classic,
            radar: -1,
            scan: 0xFF00_00FF_u32 as i32,
            bullet: -1,
            set_parts,
        };
        // None set, an older host, only the scan: nothing to report.
        assert_eq!(set_colors(None), None);
        assert_eq!(set_colors(Some(&colors(0, 0xFF00_0000_u32 as i32))), None);
        assert_eq!(set_colors(Some(&colors(8, classic))), None);
        // Body and radar set: those, the gun left out.
        assert_eq!(
            set_colors(Some(&colors(5, 0xFF12_3456_u32 as i32))),
            Some(BotColors {
                body: Some("#123456".into()),
                gun: None,
                radar: Some("#ffffff".into()),
            })
        );

        let rules = Rules {
            rounds: 2,
            width: 800,
            height: 600,
            gun_cooling_rate: 0.1,
            inactivity_time: 450,
            sentry_border: 100,
            hide_enemy_names: false,
        };
        let snapshot = |round: i32, turn: i64, robot0: v1::Colors| {
            server_message::Msg::TurnSnapshot(v1::TurnSnapshot {
                round,
                turn,
                robots: vec![
                    v1::RobotSnapshot {
                        colors: Some(robot0),
                        ..Default::default()
                    },
                    v1::RobotSnapshot {
                        colors: Some(colors(0, classic)),
                        ..Default::default()
                    },
                ],
                bullets: vec![],
            })
        };
        let mut observer = Observer::new(&rules);
        observer.observe(&snapshot(0, 0, colors(0, classic)));
        observer.observe(&snapshot(0, 1, colors(1, 0xFF00_0000_u32 as i32)));
        // A robot that changes its colours later: the first ones count.
        observer.observe(&snapshot(0, 2, colors(1, 0xFF00_00FF_u32 as i32)));
        assert_eq!(
            observer.colors,
            [
                Some(BotColors {
                    body: Some("#000000".into()),
                    ..Default::default()
                }),
                None
            ]
        );
    }

    #[test]
    fn the_sandbox_policy_refuses_none_unless_allowed() {
        assert!(check_sandbox(Sandbox::None, false).is_err());
        assert!(check_sandbox(Sandbox::None, true).is_ok());
    }

    #[test]
    fn the_engine_check_compares_versions_and_rules() {
        let mut info = Info {
            api: 1,
            engine_version: ENGINE_VERSION,
            min_client: "0.0.0".into(),
            paused: false,
            github_client_id: String::new(),
            deterministic_rules: DETERMINISTIC_RULES,
            divisions: vec![],
            downloads: String::new(),
        };
        assert!(check_engine(&info).is_ok());
        info.engine_version += 1;
        assert!(check_engine(&info).unwrap_err().contains("engine version"));
        info.engine_version -= 1;
        info.deterministic_rules += 1;
        assert!(check_engine(&info).is_err());
    }

    #[test]
    fn a_failed_description_names_its_robot() {
        let names = ["a.B 1.0".to_string(), "c.D".to_string()];
        assert_eq!(
            named_robot("robot c.D not found in [x.jar]", &names),
            Some(1)
        );
        assert_eq!(named_robot("no Java runtime", &names), None);
    }
}

#[cfg(test)]
mod constants {
    /// The Lambdas take these from `rumble-core`, which can't see the engine's or the
    /// server's: they must not drift apart.
    #[test]
    fn core_constants_match_the_engine() {
        assert_eq!(
            rumble_core::config::DEFAULT_ENGINE_VERSION,
            robocode_s_engine::ENGINE_VERSION
        );
        assert_eq!(
            rumble_core::config::DETERMINISTIC_RULES,
            robocode_s_server::DETERMINISTIC_RULES
        );
    }
}
