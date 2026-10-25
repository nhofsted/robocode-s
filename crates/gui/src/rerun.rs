// SPDX-License-Identifier: EPL-2.0

//! Running a rumble battle again from a downloaded track ([`TrackBattle`]), for everything
//! the track leaves out: names, scores, console output, debug properties. Rumble battles
//! are deterministic (ADR 0017): the same jars, in the same order, with the same seed,
//! rules and engine version, fight the same battle. So it runs as the rumble ran it, with
//! exactly those jars and no data files, and fast up to the track's round, where it pauses.
//!
//! A jar is taken from the rumble's jar cache, else from the robot folders (the same file,
//! by its hash: an author's own robot), else downloaded from the rumble into its cache.

use crate::app::Setup;
use crate::track::{TrackBattle, TrackRobot};
use robocode_s_engine::ENGINE_VERSION;
use rumble_client::http::{Api, sha256_hex};
use rumble_client::jars::JarCache;
use rumble_core::api::WorkBot;
use rumble_core::config::DivisionConfig;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The largest bot data file read: a bot's versions and history.
const BOT_FILE_LIMIT: u64 = 8 << 20;

/// The robots of a battle run again, as [`Setup::rumble`]: their jars are found (and
/// fetched from `server`) when it starts.
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

/// The setup that runs `battle` again (from the rumble at `server`) and its seed: `base`
/// with the battle's robots and rules, deterministic, pausing at the track's round. A rule
/// the battle leaves out is the rumble's, the gun cooling the track's.
pub fn setup(
    base: &Setup,
    battle: &TrackBattle,
    gun_cooling_milli: i64,
    server: String,
) -> (Setup, i64) {
    let rumble = DivisionConfig::default();
    let mut setup = base.clone();
    setup.robots = battle.robots.iter().map(TrackRobot::name).collect();
    setup.seed.clone_from(&battle.seed);
    setup.rounds = battle.rounds;
    (setup.width, setup.height) = (battle.width, battle.height);
    setup.classic = false;
    setup.record = String::new();
    setup.paint = false;
    setup.rules.gun_cooling_rate = battle
        .gun_cooling_rate
        .unwrap_or(gun_cooling_milli as f64 / 1000.0);
    setup.rules.inactivity_time = battle
        .inactivity_time
        .unwrap_or(i64::from(rumble.inactivity_time));
    setup.rules.sentry_border = battle.sentry_border.unwrap_or(rumble.sentry_border as i32);
    setup.rules.hide_enemy_names = battle.hide_enemy_names.unwrap_or(rumble.hide_enemy_names);
    setup.rumble = Some(RumbleRobots {
        server,
        robots: battle.robots.clone(),
    });
    setup.pause_at_round = (battle.round > 0).then_some(battle.round);
    (setup, battle.seed.parse().unwrap_or_default())
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
    fn the_setup_is_the_battle_as_the_rumble_ran_it() {
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
        let mut base = Setup::new(&crate::settings::Settings::default());
        base.classic = true;
        base.paint = true;
        base.record = "x.rsr".into();
        let (setup, seed) = setup(&base, &battle, 100, "https://r.example".into());
        assert_eq!(seed, -42);
        assert_eq!(setup.robots, ["b.B 1.0", "a.A 1.0"]);
        assert_eq!((setup.rounds, setup.width, setup.height), (35, 800, 600));
        assert!(!setup.classic && !setup.paint && setup.record.is_empty());
        assert_eq!(setup.rules.gun_cooling_rate, 0.1);
        assert_eq!(setup.rules.inactivity_time, 450);
        assert_eq!(setup.rules.sentry_border, 100);
        assert_eq!(setup.pause_at_round, Some(3));
        assert_eq!(setup.rumble.unwrap().robots, battle.robots);
        assert!(unavailable(&battle).is_none());
        let other = TrackBattle {
            engine: ENGINE_VERSION + 1,
            ..battle
        };
        assert!(unavailable(&other).is_some());
    }
}
