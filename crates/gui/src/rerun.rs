// SPDX-License-Identifier: EPL-2.0

//! Turning a downloaded track ([`TrackBattle`]) into a full recording, for everything the
//! track leaves out: names, scores, console output, debug properties. Rumble battles are
//! deterministic (ADR 0017): the same jars, in the same order, with the same seed, rules
//! and engine version, fight the same battle. So it runs again as the rumble ran it, with
//! exactly those jars and no data files, unseen and as fast as it goes, recorded to a file
//! that then opens as any recording.
//!
//! A jar is taken from the rumble's jar cache, else from the robot folders (the same file,
//! by its hash: an author's own robot), else downloaded from the rumble into its cache.

use crate::track::{TrackBattle, TrackRobot};
use robocode_s_engine::{BattleRules, ENGINE_VERSION};
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::local::{Environment, prepare_with, run_local_battle};
use robocode_s_server::{Connection, ServerConfig};
use rumble_client::http::{Api, sha256_hex};
use rumble_client::jars::JarCache;
use rumble_core::api::WorkBot;
use rumble_core::config::DivisionConfig;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// The largest bot data file read: a bot's versions and history.
const BOT_FILE_LIMIT: u64 = 8 << 20;
/// The robots' heap, as the rumble gives them.
const MEMORY_MB: u32 = 512;
/// Full recordings not saved are removed once they are this old.
const STALE_RECORDING: Duration = Duration::from_secs(24 * 60 * 60);

/// A battle's robots and the rumble their jars come from if they aren't here.
#[derive(Clone, Debug, PartialEq)]
pub struct RumbleRobots {
    pub server: String,
    pub robots: Vec<TrackRobot>,
}

/// Why `battle` can't run here, if it can't.
pub fn unavailable(battle: &TrackBattle) -> Option<String> {
    (battle.engine != ENGINE_VERSION).then(|| {
        format!(
            "It ran with engine version {}; this robocode-s has engine version \
             {ENGINE_VERSION}, where the battle would go differently.",
            battle.engine
        )
    })
}

/// The rules `battle` ran with. A rule the battle leaves out is the rumble's, the gun
/// cooling the track's.
pub fn rules(battle: &TrackBattle, gun_cooling_milli: i64) -> BattleRules {
    let rumble = DivisionConfig::default();
    BattleRules {
        width: battle.width,
        height: battle.height,
        num_rounds: battle.rounds,
        gun_cooling_rate: battle
            .gun_cooling_rate
            .unwrap_or(gun_cooling_milli as f64 / 1000.0),
        inactivity_time: battle
            .inactivity_time
            .unwrap_or(i64::from(rumble.inactivity_time)),
        hide_enemy_names: battle.hide_enemy_names.unwrap_or(rumble.hide_enemy_names),
        sentry_border_size: battle.sentry_border.unwrap_or(rumble.sentry_border as i32),
    }
}

/// Where a full recording of the track `track` goes until it is saved: a new file in
/// `<data>/recordings`, where those older than a day are removed first.
pub fn recording_path(data: &Path, track: &Path) -> PathBuf {
    let dir = data.join("recordings");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| {
                SystemTime::now().duration_since(t).unwrap_or_default() > STALE_RECORDING
            });
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let stem = track.file_stem().unwrap_or_default().to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    dir.join(format!("{stem}-{stamp}-{}.rec", std::process::id()))
}

/// What [`record`] is doing.
pub enum Stage {
    /// Finding (or downloading) the robots' jars.
    Robots,
    /// Starting the robots, then fighting the battle: the round (0-based) and the rounds.
    Battle(i32, i32),
}

/// Runs `battle` again, robots from the rumble at `server` if need be, and records it to
/// `path`. `progress` hears what it is doing; a `cancel` stops it (and removes the file).
pub fn record(
    env: &Environment,
    server: &str,
    battle: &TrackBattle,
    gun_cooling_milli: i64,
    path: &Path,
    progress: &dyn Fn(Stage),
    cancel: &AtomicBool,
) -> Result<(), String> {
    if let Some(why) = unavailable(battle) {
        return Err(why);
    }
    progress(Stage::Robots);
    let rumble = RumbleRobots {
        server: server.to_string(),
        robots: battle.robots.clone(),
    };
    let jars = jars(&rumble, &env.data, &env.jars)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let env = Environment {
        jars,
        fresh_data: true,
        ..env.clone()
    };
    let config = ServerConfig {
        rules: rules(battle, gun_cooling_milli),
        seed: battle.seed.parse().map_err(|_| "the seed isn't a number")?,
        deterministic: true,
        record: Some(path.to_path_buf()),
        ..Default::default()
    };
    let names: Vec<String> = battle.robots.iter().map(TrackRobot::name).collect();
    progress(Stage::Battle(0, battle.rounds));
    let (host, local) = prepare_with(&env, &names, config, MEMORY_MB, None)?;
    if local.robots.len() != names.len() {
        local.remove_dir();
        return Err(format!(
            "{} robots found for the battle's {}",
            local.robots.len(),
            names.len()
        ));
    }
    let (controls, rx) = mpsc::channel();
    let mut aborted = false;
    let ran = run_local_battle(&host, &local, Some(rx), |msg| {
        if !aborted && cancel.load(Ordering::Relaxed) {
            aborted = true;
            let _ = controls.send(v1::ObserverControl {
                control: Some(v1::observer_control::Control::Abort(true)),
            });
        }
        if let server_message::Msg::TurnSnapshot(s) = msg
            && s.turn == 0
        {
            progress(Stage::Battle(s.round, battle.rounds));
        }
    });
    let failed = match ran {
        Err(e) => Some(e),
        Ok(_) if aborted => Some("cancelled".into()),
        Ok(ran) => ran
            .outcome
            .connections
            .iter()
            .zip(&names)
            .enumerate()
            .find(|(_, (c, _))| **c != Connection::Kept)
            .map(|(i, (_, name))| {
                let detail = ran.stderr.get(i).map_or("", |s| s.trim());
                format!("{name} didn't run to the end of the battle: {detail}")
            }),
    };
    match failed {
        Some(e) => {
            let _ = std::fs::remove_file(path);
            Err(e)
        }
        None => Ok(()),
    }
}

/// The jars of `rumble`'s robots, each once, in battle order: from the jar cache under
/// `data`, the robot jars `local`, or the rumble.
pub fn jars(rumble: &RumbleRobots, data: &Path, local: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let cache = JarCache::in_data_dir(data);
    let api = Api::new(&format!("{}/api/v1", rumble.server), None, ENGINE_VERSION);
    let mut hashed: Option<Vec<(String, PathBuf)>> = None;
    let mut jars: Vec<PathBuf> = Vec::new();
    for robot in &rumble.robots {
        let name = robot.name();
        if !valid_sha16(&robot.sha16) || !valid_id(&robot.bot) {
            return Err(format!("{name}: not a rumble robot"));
        }
        let jar = match cached(cache.root(), &robot.sha16) {
            Some(jar) => jar,
            None => {
                let hashed = hashed.get_or_insert_with(|| hash_jars(local));
                match hashed.iter().find(|(sha, _)| sha.starts_with(&robot.sha16)) {
                    Some((_, jar)) => jar.clone(),
                    None => download(&api, &cache, robot)?,
                }
            }
        };
        if !jars.contains(&jar) {
            jars.push(jar);
        }
    }
    Ok(jars)
}

fn valid_sha16(sha16: &str) -> bool {
    sha16.len() == 16
        && sha16
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A bot id that is safe in a URL path: a Java class name.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'$' | b'-'))
}

/// A jar in the cache (`<sha256>/<file>.jar`) whose hash starts with `sha16` and is its
/// folder's name.
fn cached(root: &Path, sha16: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    entries.flatten().find_map(|dir| {
        let sha = dir.file_name().to_str()?.to_string();
        if sha.len() != 64 || !sha.starts_with(sha16) {
            return None;
        }
        let files = std::fs::read_dir(dir.path()).ok()?;
        files.flatten().map(|f| f.path()).find(|jar| {
            jar.extension().is_some_and(|e| e == "jar")
                && std::fs::read(jar).is_ok_and(|bytes| sha256_hex(&bytes) == sha)
        })
    })
}

/// The SHA-256 of every jar file among `jars` (class folders have none).
fn hash_jars(jars: &[PathBuf]) -> Vec<(String, PathBuf)> {
    jars.iter()
        .filter(|j| j.is_file())
        .filter_map(|j| Some((sha256_hex(&std::fs::read(j).ok()?), j.clone())))
        .collect()
}

/// What a bot data file (`/data/v1/bots/<id>.json`) says of the bot's versions.
#[derive(Deserialize)]
struct BotFile {
    versions: Vec<BotVersion>,
}

#[derive(Deserialize)]
struct BotVersion {
    version: String,
    sha256: String,
    size: u64,
}

/// Fetches `robot`'s jar from the rumble into the cache: its hash and size from the bot's
/// data file, then the jar, checked against both.
fn download(api: &Api, cache: &JarCache, robot: &TrackRobot) -> Result<PathBuf, String> {
    let name = robot.name();
    let server = api.origin().to_string();
    let bytes = api
        .download(&format!("/data/v1/bots/{}.json", robot.bot), BOT_FILE_LIMIT)
        .map_err(|e| format!("{name}: can't get it from {server}: {}", e.message()))?;
    let file: BotFile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{name}: {server} sent an unexpected bot file: {e}"))?;
    let Some(version) = file
        .versions
        .into_iter()
        .find(|v| v.sha256.starts_with(&robot.sha16) && v.version == robot.version)
    else {
        return Err(format!(
            "{name}: {server} doesn't have this version (taken down, or from another rumble?)"
        ));
    };
    let bot = WorkBot {
        id: robot.bot.clone(),
        version: version.version,
        vid: 0,
        url: format!("/jars/{}.jar", version.sha256),
        sha256: version.sha256,
        size: version.size,
        data: None,
    };
    cache
        .get(&bot, &|b| {
            api.download(&b.url, b.size + 1)
                .map_err(|e| e.message().to_string())
        })
        .map_err(|e| format!("{name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn robot(bot: &str, sha: &str) -> TrackRobot {
        TrackRobot {
            bot: bot.into(),
            version: "1.0".into(),
            sha16: sha[..16].into(),
        }
    }

    #[test]
    fn jars_come_from_the_cache_and_the_robot_folders_by_hash() {
        let dir = std::env::temp_dir().join(format!("robocodes-rerun-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (b"jar a".as_slice(), b"jar b".as_slice());
        let (sha_a, sha_b) = (sha256_hex(a), sha256_hex(b));
        let cached = dir.join("rumble/jars").join(&sha_a).join("a.A_1.0.jar");
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, a).unwrap();
        let mine = dir.join("robots/b.jar");
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::write(&mine, b).unwrap();
        let rumble = RumbleRobots {
            // Nothing is downloaded: no server.
            server: "http://127.0.0.1:9".into(),
            robots: vec![
                robot("a.A", &sha_a),
                robot("b.B", &sha_b),
                robot("a.A", &sha_a),
            ],
        };
        let local = [dir.join("robots"), mine.clone()];
        assert_eq!(jars(&rumble, &dir, &local).unwrap(), [cached, mine]);
        // A jar that isn't there, from a rumble that isn't either.
        let missing = RumbleRobots {
            robots: vec![robot("c.C", &sha256_hex(b"jar c"))],
            ..rumble
        };
        let e = jars(&missing, &dir, &local).unwrap_err();
        assert!(
            e.starts_with("c.C 1.0: can't get it from http://127.0.0.1:9"),
            "{e}"
        );
        let bad = RumbleRobots {
            robots: vec![robot("../x", &sha_a)],
            ..missing
        };
        assert!(
            jars(&bad, &dir, &local)
                .unwrap_err()
                .contains("not a rumble robot")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rules_are_the_rumbles() {
        let battle = TrackBattle {
            schema: 1,
            engine: ENGINE_VERSION,
            seed: "-42".into(),
            round: 3,
            rounds: 35,
            width: 800,
            height: 600,
            gun_cooling_rate: None,
            inactivity_time: None,
            sentry_border: None,
            hide_enemy_names: None,
            robots: vec![robot("b.B", &"1".repeat(64)), robot("a.A", &"2".repeat(64))],
        };
        let r = rules(&battle, 100);
        assert_eq!((r.width, r.height, r.num_rounds), (800, 600, 35));
        assert_eq!(r.gun_cooling_rate, 0.1);
        assert_eq!((r.inactivity_time, r.sentry_border_size), (450, 100));
        assert!(!r.hide_enemy_names);
        assert!(unavailable(&battle).is_none());
        let other = TrackBattle {
            engine: ENGINE_VERSION + 1,
            ..battle
        };
        assert!(unavailable(&other).is_some());
        let dir = std::env::temp_dir().join(format!("robocodes-rec-{}", std::process::id()));
        let path = recording_path(&dir, Path::new("/x/A_vs_B_round4_a-0.track"));
        assert_eq!(path.parent(), Some(dir.join("recordings").as_path()));
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("A_vs_B_round4_a-0-") && name.ends_with(".rec"),
            "{name}"
        );
    }
}
