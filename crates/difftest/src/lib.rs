// SPDX-License-Identifier: EPL-2.0

//! Replays classic Robocode recordings in the robocode-s engine.
//!
//! A recording (format 3, `tools/difftest`) holds, per turn, the command state classic
//! loaded for every robot, and the resulting robot states, bullets, remaining values and
//! events. The replay feeds the same commands into the engine and compares everything,
//! bit for bit. It stops at the first difference and describes it.

pub mod live;

use flate2::read::GzDecoder;
use robocode_s_engine::{
    Battle, BattleRules, BulletCommand, Commands, Event, RobotSetup, TeamMessage,
};
use robocode_s_protocol::v1;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// What a successful replay covered.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub rounds: usize,
    pub turns: usize,
    pub events: usize,
}

/// The first difference between classic and the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub round: i64,
    pub turn: i64,
    pub message: String,
    /// Set when classic's recording shows a known race before the mismatch, in the same
    /// round (see [`classic_race`]): the difference is then not a robocode-s error.
    pub race: Option<String>,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.race {
            Some(race) => write!(
                f,
                "{RACE}: {race}; round {} turn {}: {}",
                self.round, self.turn, self.message
            ),
            None => write!(
                f,
                "round {} turn {}: {}",
                self.round, self.turn, self.message
            ),
        }
    }
}

/// How a [`Mismatch`] caused by a known classic race starts when printed.
pub const RACE: &str = "classic race";

/// A known race in classic, visible in its recording, in `round` up to `turn`: a dead
/// robot that regained energy. Its bullet hit after its death and its thread hadn't
/// finished yet, so it can even fire again with its last commands (engine-spec §15.3).
/// robocode-s finishes dead robots at once, so the battles diverge from there.
pub fn classic_race(lines: &[Value], round: i64, turn: i64) -> Option<String> {
    lines
        .iter()
        .filter(|l| {
            l["round"].as_i64() == Some(round) && l["turn"].as_i64().is_some_and(|t| t <= turn)
        })
        .find_map(|l| {
            let robots = l["robots"].as_array()?;
            robots.iter().enumerate().find_map(|(i, r)| {
                (r["state"] == "DEAD" && number(&r["energy"]) > 0.0).then(|| {
                    format!(
                        "robot {i} was dead with energy {} at turn {}",
                        number(&r["energy"]),
                        l["turn"]
                    )
                })
            })
        })
}

/// Replays a recording file (`.jsonl` or `.jsonl.gz`).
pub fn replay_file(path: &Path) -> Result<Summary, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut lines = Vec::new();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        lines.push(
            serde_json::from_str::<Value>(&line).map_err(|e| format!("{}: {e}", path.display()))?,
        );
    }
    replay(&lines).map_err(|m| m.to_string())
}

/// Replays parsed recording lines.
pub fn replay(lines: &[Value]) -> Result<Summary, Mismatch> {
    replay_turns(lines).map_err(|mut m| {
        m.race = classic_race(lines, m.round, m.turn);
        m
    })
}

fn replay_turns(lines: &[Value]) -> Result<Summary, Mismatch> {
    let header = lines
        .first()
        .filter(|h| h["type"] == "header")
        .ok_or_else(|| fail(0, 0, "no header"))?;
    if header["format"].as_i64() != Some(3) {
        return Err(fail(
            0,
            0,
            &format!("unsupported format {}", header["format"]),
        ));
    }
    let mut battle = Battle::new(
        rules(header),
        setups(header),
        header["seed"].as_i64().unwrap_or(0),
    );
    let mut summary = Summary::default();
    let mut started = false;
    for line in &lines[1..] {
        let round = line["round"].as_i64().unwrap_or(-1);
        let turn = line["turn"].as_i64().unwrap_or(-1);
        match line["type"].as_str() {
            Some("roundStart") => {
                if started {
                    if !battle.is_round_over() {
                        return Err(fail(
                            round,
                            turn,
                            "classic started a new round, the engine's round is not over",
                        ));
                    }
                    battle.finish_round();
                }
                started = true;
                battle.start_round();
                summary.rounds += 1;
                compare_state(&battle, line).map_err(|m| fail(round, turn, &m))?;
            }
            Some("turn") => {
                if battle.is_round_over() {
                    return Err(fail(
                        round,
                        turn,
                        "the engine's round ended, classic continues",
                    ));
                }
                let commands = line["commands"]
                    .as_array()
                    .ok_or_else(|| fail(round, turn, "no commands"))?;
                for (i, c) in commands.iter().enumerate() {
                    if !c.is_null() {
                        battle.set_commands(i, parse_commands(c));
                    }
                }
                battle.step();
                summary.turns += 1;
                if battle.time != turn {
                    return Err(fail(
                        round,
                        turn,
                        &format!("engine is at turn {}", battle.time),
                    ));
                }
                // Classic reports the remaining values before the turn's wakeup, in which
                // robot threads may drain or disable their robot; the snapshot and the
                // events come after it.
                compare_remaining(&battle, line).map_err(|m| fail(round, turn, &m))?;
                apply_actions(&mut battle, line).map_err(|m| fail(round, turn, &m))?;
                compare_state(&battle, line).map_err(|m| fail(round, turn, &m))?;
                summary.events +=
                    compare_events(&mut battle, line).map_err(|m| fail(round, turn, &m))?;
            }
            Some("results") => {
                if !battle.is_round_over() {
                    return Err(fail(
                        round,
                        turn,
                        "classic finished, the engine's round is not over",
                    ));
                }
            }
            other => return Err(fail(round, turn, &format!("unknown line type {other:?}"))),
        }
    }
    Ok(summary)
}

fn fail(round: i64, turn: i64, message: &str) -> Mismatch {
    Mismatch {
        round,
        turn,
        message: message.to_string(),
        race: None,
    }
}

/// The battle rules from a recording header.
pub fn rules(h: &Value) -> BattleRules {
    BattleRules {
        width: h["width"].as_i64().unwrap_or(800) as i32,
        height: h["height"].as_i64().unwrap_or(600) as i32,
        num_rounds: h["rounds"].as_i64().unwrap_or(10) as i32,
        gun_cooling_rate: number(&h["gunCoolingRate"]),
        inactivity_time: h["inactivityTime"].as_i64().unwrap_or(450),
        hide_enemy_names: h["hideEnemyNames"].as_bool().unwrap_or(false),
        sentry_border_size: h["sentryBorderSize"].as_i64().unwrap_or(100) as i32,
    }
}

/// The robots from a recording header (format 3).
pub fn setups(h: &Value) -> Vec<RobotSetup> {
    h["robotInfo"]
        .as_array()
        .map(|robots| {
            robots
                .iter()
                .map(|r| {
                    let name = r["name"].as_str().unwrap_or("").to_string();
                    let team = r["teamIndex"]
                        .as_i64()
                        .filter(|&t| t >= 0)
                        .map(|t| t as usize);
                    RobotSetup {
                        full_class_name: name.split(' ').next().unwrap_or("").to_string(),
                        name,
                        advanced: r["advanced"].as_bool().unwrap_or(false),
                        team_robot: r["teamRobot"].as_bool().unwrap_or(false),
                        droid: r["droid"].as_bool().unwrap_or(false),
                        sentry: r["sentry"].as_bool().unwrap_or(false),
                        leader: r["leader"].as_bool().unwrap_or(false),
                        team,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A double from the recording: a JSON number, or "NaN" / "Infinity" / "-Infinity".
pub fn number(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match s.as_str() {
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => f64::NAN,
        },
        _ => f64::NAN,
    }
}

/// The JSON form of a double, as the oracle writes it.
pub fn json_number(d: f64) -> Value {
    if d.is_finite() {
        json!(d)
    } else if d.is_nan() {
        json!("NaN")
    } else if d > 0.0 {
        json!("Infinity")
    } else {
        json!("-Infinity")
    }
}

/// Recorded commands (`OracleHooks.commandsLoaded`).
pub fn parse_commands(c: &Value) -> Commands {
    let bullets = c["bullets"]
        .as_array()
        .map(|bs| {
            bs.iter()
                .map(|b| BulletCommand {
                    id: b["id"].as_i64().unwrap_or(0) as i32,
                    power: number(&b["power"]),
                    assist_valid: b["assistValid"].as_bool().unwrap_or(false),
                    assist_angle: number(&b["assistAngle"]),
                    consumed: b["consumed"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let messages = c["messages"]
        .as_array()
        .map(|ms| {
            ms.iter()
                .map(|m| TeamMessage {
                    sender: m["sender"].as_str().unwrap_or("").to_string(),
                    recipient: m["recipient"]
                        .as_str()
                        .filter(|r| *r != "null")
                        .map(str::to_string),
                    bytes: hex(m["bytes"].as_str().unwrap_or("")),
                })
                .collect()
        })
        .unwrap_or_default();
    Commands {
        body_turn_remaining: number(&c["bodyTurn"]),
        gun_turn_remaining: number(&c["gunTurn"]),
        radar_turn_remaining: number(&c["radarTurn"]),
        distance_remaining: number(&c["distance"]),
        adjust_gun_for_body_turn: c["adjustGunForBody"].as_bool().unwrap_or(false),
        adjust_radar_for_gun_turn: c["adjustRadarForGun"].as_bool().unwrap_or(false),
        adjust_radar_for_body_turn: c["adjustRadarForBody"].as_bool().unwrap_or(false),
        max_turn_rate: number(&c["maxTurnRate"]),
        max_velocity: number(&c["maxVelocity"]),
        scan: c["scan"].as_bool().unwrap_or(false),
        io: c["io"].as_bool().unwrap_or(false),
        bullets,
        messages,
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

/// Exact comparison of doubles; NaN equals NaN.
pub fn same(expected: f64, actual: f64) -> bool {
    expected.to_bits() == actual.to_bits() || (expected.is_nan() && actual.is_nan())
}

pub fn describe(d: f64) -> String {
    format!("{d:?} (bits {:016x})", d.to_bits())
}

/// What robot threads did to their robot while handling the turn (`actions`, only
/// present when something happened): `drainEnergy` or `disable:<reason>`.
fn apply_actions(battle: &mut Battle, line: &Value) -> Result<(), String> {
    let Some(actions) = line["actions"].as_array() else {
        return Ok(());
    };
    for (i, robot) in actions.iter().enumerate() {
        for action in robot.as_array().into_iter().flatten() {
            match action.as_str() {
                Some("drainEnergy") => battle.drain_energy(i),
                Some(a) if a.starts_with("disable:") => battle.disable(i),
                _ => return Err(format!("robot {i}: unknown action {action}")),
            }
        }
    }
    Ok(())
}

fn compare_state(battle: &Battle, line: &Value) -> Result<(), String> {
    let robots = line["robots"].as_array().ok_or("no robots")?;
    if robots.len() != battle.robots.len() {
        return Err(format!(
            "{} robots recorded, engine has {}",
            robots.len(),
            battle.robots.len()
        ));
    }
    for (i, (r, e)) in robots.iter().zip(&battle.robots).enumerate() {
        if r["state"].as_str() != Some(e.state.name()) {
            return Err(format!(
                "robot {i} ({}) state: classic {}, engine {}",
                e.setup.name,
                r["state"],
                e.state.name()
            ));
        }
        let fields = [
            ("x", e.x),
            ("y", e.y),
            ("energy", e.energy),
            ("velocity", e.velocity),
            ("bodyHeading", e.body_heading),
            ("gunHeading", e.gun_heading),
            ("radarHeading", e.radar_heading),
            ("gunHeat", e.gun_heat),
            ("score", e.current_score()),
            ("totalScore", e.total_score()),
        ];
        for (name, actual) in fields {
            if dead_energy_race(r, name) {
                continue;
            }
            let expected = number(&r[name]);
            if !same(expected, actual) {
                return Err(format!(
                    "robot {i} ({}) {name}: classic {}, engine {}",
                    e.setup.name,
                    describe(expected),
                    describe(actual)
                ));
            }
        }
    }
    let bullets = line["bullets"].as_array().ok_or("no bullets")?;
    if bullets.len() != battle.bullets.len() {
        return Err(format!(
            "{} bullets recorded, engine has {}: classic {}, engine {}",
            bullets.len(),
            battle.bullets.len(),
            Value::Array(bullets.clone()),
            bullets_json(battle)
        ));
    }
    for (k, (b, e)) in bullets.iter().zip(&battle.bullets).enumerate() {
        let victim = e.victim.map_or(-1, |v| v as i64);
        let ints = [
            ("owner", e.owner as i64),
            ("id", i64::from(e.id)),
            ("victim", victim),
        ];
        for (name, actual) in ints {
            if b[name].as_i64() != Some(actual) {
                return Err(format!(
                    "bullet {k} {name}: classic {}, engine {actual}",
                    b[name]
                ));
            }
        }
        if b["state"].as_str() != Some(e.state.name()) {
            return Err(format!(
                "bullet {k} state: classic {}, engine {}",
                b["state"],
                e.state.name()
            ));
        }
        for (name, actual) in [
            ("x", e.x),
            ("y", e.y),
            ("heading", e.heading),
            ("power", e.power),
        ] {
            let expected = number(&b[name]);
            if !same(expected, actual) {
                return Err(format!(
                    "bullet {k} {name}: classic {}, engine {}",
                    describe(expected),
                    describe(actual)
                ));
            }
        }
    }
    Ok(())
}

fn bullets_json(battle: &Battle) -> Value {
    Value::Array(
        battle
            .bullets
            .iter()
            .map(|b| {
                json!({"owner": b.owner, "id": b.id, "state": b.state.name(), "x": json_number(b.x),
                       "y": json_number(b.y), "victim": b.victim.map_or(-1, |v| v as i64)})
            })
            .collect(),
    )
}

fn compare_remaining(battle: &Battle, line: &Value) -> Result<(), String> {
    let remaining = line["remaining"].as_array().ok_or("no remaining values")?;
    for (i, (r, e)) in remaining.iter().zip(&battle.robots).enumerate() {
        if r.is_null() {
            continue;
        }
        let c = &e.commands;
        for (name, actual) in [
            ("bodyTurn", c.body_turn_remaining),
            ("gunTurn", c.gun_turn_remaining),
            ("radarTurn", c.radar_turn_remaining),
            ("distance", c.distance_remaining),
        ] {
            let expected = number(&r[name]);
            if !same(expected, actual) {
                return Err(format!(
                    "robot {i} ({}) remaining {name}: classic {}, engine {}",
                    e.setup.name,
                    describe(expected),
                    describe(actual)
                ));
            }
        }
    }
    Ok(())
}

/// Compares and consumes the events generated this turn; returns how many there were.
fn compare_events(battle: &mut Battle, line: &Value) -> Result<usize, String> {
    let recorded = line["events"].as_array().ok_or("no events")?;
    let mut count = 0;
    for (i, expected) in recorded.iter().enumerate() {
        let actual: Vec<Value> = battle.take_events(i).iter().map(event_json).collect();
        let expected = expected.as_array().cloned().unwrap_or_default();
        if !events_equal(&expected, &actual) {
            let mut message = format!(
                "robot {i} ({}) events differ:\n  classic:",
                battle.robots[i].setup.name
            );
            for e in &expected {
                let _ = write!(message, "\n    {e}");
            }
            message.push_str("\n  engine:");
            for e in &actual {
                let _ = write!(message, "\n    {e}");
            }
            return Err(message);
        }
        count += expected.len();
    }
    Ok(count)
}

/// Equal JSON, with numbers compared as exact doubles.
pub fn events_equal(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_equal(x, y))
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(p), Some(q)) => p == q,
            _ => same(
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            ),
        },
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| values_equal(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => events_equal(x, y),
        _ => a == b,
    }
}

fn bullet_json(b: &robocode_s_engine::BulletInfo) -> Value {
    json!({
        "id": b.id,
        "owner": b.owner,
        "victim": b.victim.clone().unwrap_or_else(|| "null".to_string()),
        "heading": json_number(b.heading),
        "x": json_number(b.x),
        "y": json_number(b.y),
        "power": json_number(b.power),
        "active": b.active,
    })
}

/// The oracle's JSON form of an event (`OracleHooks.describe`).
pub fn event_json(e: &Event) -> Value {
    match e {
        Event::ScannedRobot {
            name,
            energy,
            bearing,
            distance,
            heading,
            velocity,
            sentry,
        } => json!({"type": "ScannedRobotEvent", "name": name, "energy": json_number(*energy),
                    "bearing": json_number(*bearing), "distance": json_number(*distance),
                    "heading": json_number(*heading), "velocity": json_number(*velocity), "sentry": sentry}),
        Event::HitWall { bearing } => {
            json!({"type": "HitWallEvent", "bearing": json_number(*bearing)})
        }
        Event::HitRobot {
            name,
            bearing,
            energy,
            at_fault,
        } => json!({"type": "HitRobotEvent", "name": name, "bearing": json_number(*bearing),
                    "energy": json_number(*energy), "atFault": at_fault}),
        Event::HitByBullet { bearing, bullet } => {
            json!({"type": "HitByBulletEvent", "bearing": json_number(*bearing), "bullet": bullet_json(bullet)})
        }
        Event::BulletHit {
            name,
            energy,
            bullet,
        } => json!({"type": "BulletHitEvent", "name": name,
                    "energy": json_number(*energy), "bullet": bullet_json(bullet)}),
        Event::BulletHitBullet { bullet, hit_bullet } => json!({"type": "BulletHitBulletEvent",
                    "bullet": bullet_json(bullet), "hitBullet": bullet_json(hit_bullet)}),
        Event::BulletMissed { bullet } => {
            json!({"type": "BulletMissedEvent", "bullet": bullet_json(bullet)})
        }
        Event::RobotDeath { name } => json!({"type": "RobotDeathEvent", "name": name}),
        Event::Death => json!({"type": "DeathEvent"}),
        Event::Win => json!({"type": "WinEvent"}),
        Event::RoundEnded {
            round,
            turns,
            total_turns,
        } => {
            json!({"type": "RoundEndedEvent", "round": round, "turns": turns, "totalTurns": total_turns})
        }
        Event::BattleEnded { aborted, results } => {
            json!({"type": "BattleEndedEvent", "aborted": aborted,
                    "rank": results.rank, "score": (results.score + 0.5) as i64,
                    "survival": (results.survival + 0.5) as i64, "firsts": results.firsts})
        }
        Event::SkippedTurn { turn } => json!({"type": "SkippedTurnEvent", "skippedTurn": turn}),
    }
}

// ---------------------------------------------------------------- shared by the server tests

/// Reads all lines of a recording (`.jsonl` or `.jsonl.gz`).
pub fn load(path: &Path) -> Vec<Value> {
    let file = std::fs::File::open(path).unwrap();
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    BufReader::new(reader)
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect()
}

/// One round of a recording: its start line and turn lines (index = turn - 1).
pub struct Round {
    pub start: Value,
    pub turns: Vec<Value>,
}

/// The recording's rounds.
pub fn rounds(lines: &[Value]) -> Vec<Round> {
    let mut rounds: Vec<Round> = Vec::new();
    for line in lines {
        match line["type"].as_str() {
            Some("roundStart") => rounds.push(Round {
                start: line.clone(),
                turns: Vec::new(),
            }),
            Some("turn") => rounds.last_mut().unwrap().turns.push(line.clone()),
            _ => {}
        }
    }
    rounds
}

/// All recorded states in order: each round's start, then its turns.
pub fn states(rounds: &[Round]) -> Vec<&Value> {
    rounds
        .iter()
        .flat_map(|r| std::iter::once(&r.start).chain(r.turns.iter()))
        .collect()
}

fn proto_bullet_json(b: &Option<v1::Bullet>) -> Value {
    let b = b.clone().unwrap_or_default();
    json!({"id": b.bullet_id, "owner": b.owner, "victim": b.victim.unwrap_or_else(|| "null".into()),
           "heading": json_number(b.heading.unwrap_or_default()), "x": json_number(b.x.unwrap_or_default()), "y": json_number(b.y.unwrap_or_default()),
           "power": json_number(b.power.unwrap_or_default()), "active": b.active})
}

/// The oracle's JSON form of a protocol event.
pub fn proto_event_json(e: &v1::Event) -> Value {
    use v1::event::Kind;
    match e.kind.as_ref().unwrap() {
        Kind::ScannedRobot(s) => {
            json!({"type": "ScannedRobotEvent", "name": s.name, "energy": json_number(s.energy.unwrap_or_default()),
            "bearing": json_number(s.bearing.unwrap_or_default()), "distance": json_number(s.distance.unwrap_or_default()),
            "heading": json_number(s.heading.unwrap_or_default()), "velocity": json_number(s.velocity.unwrap_or_default()), "sentry": s.sentry})
        }
        Kind::HitWall(h) => {
            json!({"type": "HitWallEvent", "bearing": json_number(h.bearing.unwrap_or_default())})
        }
        Kind::HitRobot(h) => {
            json!({"type": "HitRobotEvent", "name": h.name, "bearing": json_number(h.bearing.unwrap_or_default()),
            "energy": json_number(h.energy.unwrap_or_default()), "atFault": h.at_fault})
        }
        Kind::HitByBullet(h) => {
            json!({"type": "HitByBulletEvent", "bearing": json_number(h.bearing.unwrap_or_default()),
            "bullet": proto_bullet_json(&h.bullet)})
        }
        Kind::BulletHit(h) => {
            json!({"type": "BulletHitEvent", "name": h.name, "energy": json_number(h.energy.unwrap_or_default()),
            "bullet": proto_bullet_json(&h.bullet)})
        }
        Kind::BulletHitBullet(h) => {
            json!({"type": "BulletHitBulletEvent", "bullet": proto_bullet_json(&h.bullet),
            "hitBullet": proto_bullet_json(&h.hit_bullet)})
        }
        Kind::BulletMissed(h) => {
            json!({"type": "BulletMissedEvent", "bullet": proto_bullet_json(&h.bullet)})
        }
        Kind::RobotDeath(d) => json!({"type": "RobotDeathEvent", "name": d.name}),
        Kind::Death(_) => json!({"type": "DeathEvent"}),
        Kind::Win(_) => json!({"type": "WinEvent"}),
        Kind::RoundEnded(r) => {
            json!({"type": "RoundEndedEvent", "round": r.round, "turns": r.turns,
            "totalTurns": r.total_turns})
        }
        Kind::BattleEnded(b) => {
            let r = b.results.clone().unwrap_or_default();
            json!({"type": "BattleEndedEvent", "aborted": b.aborted, "rank": r.rank,
                   "score": (r.score.unwrap_or_default() + 0.5) as i64, "survival": (r.survival.unwrap_or_default() + 0.5) as i64, "firsts": r.firsts})
        }
        Kind::SkippedTurn(s) => json!({"type": "SkippedTurnEvent", "skippedTurn": s.turn}),
    }
}

/// A dead robot's energy is a race in classic, so it isn't compared. A bullet it fired
/// before dying still earns it the hit bonus until its thread finishes (`updateEnergy`
/// checks `isExecFinishedAndDisabled`). Classic doesn't wait for dead robots, so robots
/// that save data in `onDeath` can still gain energy several turns later. robocode-s
/// finishes them at once. If such a robot fires again, the difference shows up elsewhere
/// and is reported as a [`classic_race`] (engine-spec §15.3).
fn dead_energy_race(recorded: &Value, field: &str) -> bool {
    field == "energy" && recorded["state"].as_str() == Some("DEAD")
}

/// Compares an observer snapshot with a recorded state line.
pub fn compare_snapshot(snapshot: &v1::TurnSnapshot, line: &Value) -> Result<(), String> {
    let at = format!("round {} turn {}", snapshot.round, snapshot.turn);
    if line["turn"].as_i64() != Some(snapshot.turn)
        || line["round"].as_i64() != Some(i64::from(snapshot.round))
    {
        return Err(format!(
            "{at}: snapshot out of step with the recording ({}/{})",
            line["round"], line["turn"]
        ));
    }
    let states = ["ACTIVE", "HIT_WALL", "HIT_ROBOT", "DEAD"];
    for (i, (r, e)) in line["robots"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&snapshot.robots)
        .enumerate()
    {
        if r["state"].as_str() != Some(states[e.state as usize]) {
            return Err(format!(
                "{at}: robot {i} state: classic {}, server {}",
                r["state"], states[e.state as usize]
            ));
        }
        for (name, actual) in [
            ("x", e.x.unwrap_or_default()),
            ("y", e.y.unwrap_or_default()),
            ("energy", e.energy.unwrap_or_default()),
            ("velocity", e.velocity.unwrap_or_default()),
            ("bodyHeading", e.body_heading.unwrap_or_default()),
            ("gunHeading", e.gun_heading.unwrap_or_default()),
            ("radarHeading", e.radar_heading.unwrap_or_default()),
            ("gunHeat", e.gun_heat.unwrap_or_default()),
            ("score", e.score.unwrap_or_default()),
            ("totalScore", e.total_score.unwrap_or_default()),
        ] {
            if dead_energy_race(r, name) {
                continue;
            }
            if !same(number(&r[name]), actual) {
                return Err(format!(
                    "{at}: robot {i} {name}: classic {}, server {}",
                    describe(number(&r[name])),
                    describe(actual)
                ));
            }
        }
    }
    let bullets = line["bullets"].as_array().unwrap();
    if bullets.len() != snapshot.bullets.len() {
        return Err(format!(
            "{at}: {} bullets recorded, server sent {}",
            bullets.len(),
            snapshot.bullets.len()
        ));
    }
    for (k, (b, e)) in bullets.iter().zip(&snapshot.bullets).enumerate() {
        let ok = b["owner"].as_i64() == Some(i64::from(e.owner))
            && b["id"].as_i64() == Some(i64::from(e.bullet_id))
            && b["victim"].as_i64() == Some(i64::from(e.victim))
            && same(number(&b["x"]), e.x.unwrap_or_default())
            && same(number(&b["y"]), e.y.unwrap_or_default())
            && same(number(&b["heading"]), e.heading.unwrap_or_default())
            && same(number(&b["power"]), e.power.unwrap_or_default());
        if !ok {
            return Err(format!(
                "{at}: bullet {k} differs: classic {b}, server {e:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod race_tests {
    use super::classic_race;
    use serde_json::json;

    fn turn(round: i64, turn: i64, state: &str, energy: f64) -> serde_json::Value {
        json!({"type": "turn", "round": round, "turn": turn,
               "robots": [{"state": "ACTIVE", "energy": 50.0}, {"state": state, "energy": energy}]})
    }

    #[test]
    fn a_dead_robot_with_energy_is_a_race_from_that_turn_in_that_round() {
        let lines = vec![
            turn(0, 10, "ACTIVE", 1.0),
            turn(0, 11, "DEAD", 0.0),
            turn(0, 12, "DEAD", 7.5),
            turn(1, 5, "ACTIVE", 100.0),
        ];
        assert_eq!(classic_race(&lines, 0, 11), None);
        assert!(classic_race(&lines, 0, 12).unwrap().contains("robot 1"));
        assert!(classic_race(&lines, 0, 40).is_some());
        assert_eq!(
            classic_race(&lines, 1, 40),
            None,
            "only the same round counts"
        );
    }
}
