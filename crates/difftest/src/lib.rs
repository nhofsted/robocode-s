// SPDX-License-Identifier: EPL-2.0

//! Replays classic Robocode recordings in the robocode-s engine.
//!
//! A recording (format 3, `tools/difftest`) holds, per turn, the command state classic
//! loaded for every robot, and the resulting robot states, bullets, remaining values and
//! events. The replay feeds the same commands into the engine and compares everything,
//! bit for bit. It stops at the first difference and describes it.

use flate2::read::GzDecoder;
use robocode_s_engine::{
    Battle, BattleRules, BulletCommand, Commands, Event, RobotSetup, TeamMessage,
};
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
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "round {} turn {}: {}",
            self.round, self.turn, self.message
        )
    }
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
                compare_state(&battle, line).map_err(|m| fail(round, turn, &m))?;
                compare_remaining(&battle, line).map_err(|m| fail(round, turn, &m))?;
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
    }
}

fn rules(h: &Value) -> BattleRules {
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

fn setups(h: &Value) -> Vec<RobotSetup> {
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
fn number(v: &Value) -> f64 {
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
fn json_number(d: f64) -> Value {
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

fn parse_commands(c: &Value) -> Commands {
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
fn same(expected: f64, actual: f64) -> bool {
    expected.to_bits() == actual.to_bits() || (expected.is_nan() && actual.is_nan())
}

fn describe(d: f64) -> String {
    format!("{d:?} (bits {:016x})", d.to_bits())
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
fn events_equal(a: &[Value], b: &[Value]) -> bool {
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
fn event_json(e: &Event) -> Value {
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
