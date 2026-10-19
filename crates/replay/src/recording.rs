// SPDX-License-Identifier: EPL-2.0

//! Builds [`Track`]s from difftest oracle recordings (`tools/difftest`, JSON Lines).

use crate::{
    ANGLE_STEPS, BULLET_ANGLE_STEPS, BulletEnd, BulletTrack, ENERGY_SCALE, POS_SCALE, RobotFrame,
    RobotState, Track, angle_units,
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

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

pub fn number(v: &Value) -> f64 {
    match v {
        Value::String(s) => s.replace("Infinity", "inf").parse().unwrap(),
        v => v.as_f64().unwrap(),
    }
}

fn state(s: &str) -> RobotState {
    match s {
        "ACTIVE" => RobotState::Active,
        "HIT_WALL" => RobotState::HitWall,
        "HIT_ROBOT" => RobotState::HitRobot,
        "DEAD" => RobotState::Dead,
        other => panic!("unknown robot state {other}"),
    }
}

fn frame(line: &Value) -> Vec<RobotFrame> {
    line["robots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| RobotFrame {
            state: state(r["state"].as_str().unwrap()),
            x: (number(&r["x"]) * POS_SCALE as f64).round() as i64,
            y: (number(&r["y"]) * POS_SCALE as f64).round() as i64,
            body: angle_units(number(&r["bodyHeading"]), ANGLE_STEPS),
            gun: angle_units(number(&r["gunHeading"]), ANGLE_STEPS),
            radar: angle_units(number(&r["radarHeading"]), ANGLE_STEPS),
            energy: (number(&r["energy"]) * ENERGY_SCALE as f64).round() as i64,
        })
        .collect()
}

/// One track per round of the recording.
pub fn tracks(lines: &[Value]) -> Vec<Track> {
    let header = lines.iter().find(|l| l["type"] == "header").unwrap();
    let mut tracks: Vec<Track> = Vec::new();
    let mut ids: HashMap<(i64, i64), usize> = HashMap::new();
    for line in lines {
        match line["type"].as_str() {
            Some("roundStart") => {
                ids.clear();
                tracks.push(Track {
                    // Classic's oracle recordings come from no robocode-s engine.
                    engine_version: 0,
                    width: header["width"].as_i64().unwrap(),
                    height: header["height"].as_i64().unwrap(),
                    gun_cooling_milli: (number(&header["gunCoolingRate"]) * 1000.0).round() as i64,
                    frames: vec![frame(line)],
                    bullets: Vec::new(),
                })
            }
            Some("turn") => {
                let track = tracks.last_mut().unwrap();
                let t = track.frames.len() as u32;
                track.frames.push(frame(line));
                for b in line["bullets"].as_array().unwrap() {
                    let s = b["state"].as_str().unwrap();
                    if s == "EXPLODED" {
                        continue; // a robot's death explosion, implied by its death
                    }
                    let key = (b["owner"].as_i64().unwrap(), b["id"].as_i64().unwrap());
                    let x = (number(&b["x"]) * POS_SCALE as f64).round() as i64;
                    let y = (number(&b["y"]) * POS_SCALE as f64).round() as i64;
                    let index = *ids.entry(key).or_insert_with(|| {
                        track.bullets.push(BulletTrack {
                            owner: key.0 as u8,
                            born: t,
                            x,
                            y,
                            heading: angle_units(number(&b["heading"]), BULLET_ANGLE_STEPS),
                            power: (number(&b["power"]) * ENERGY_SCALE as f64).round() as i64,
                            end: None,
                        });
                        track.bullets.len() - 1
                    });
                    let bullet = &mut track.bullets[index];
                    if s != "MOVING" && bullet.end.is_none() {
                        let kind = match s {
                            "HIT_WALL" => BulletEnd::Wall,
                            "HIT_BULLET" => BulletEnd::Bullet,
                            "HIT_VICTIM" => BulletEnd::Robot(b["victim"].as_i64().unwrap() as u8),
                            other => panic!("unknown bullet state {other}"),
                        };
                        bullet.end = Some((t, kind, x, y));
                    }
                }
            }
            _ => {}
        }
    }
    // The codec stores births per robot in robot order within a frame.
    for track in &mut tracks {
        track.bullets.sort_by_key(|b| (b.born, b.owner));
    }
    tracks
}

/// Bullets' recorded positions in flight, to measure how well [`BulletTrack::position`] draws
/// them: `(bullet index, frame, x, y)` in quantized units, unrounded.
pub fn bullet_positions(lines: &[Value]) -> Vec<Vec<(usize, u32, f64, f64)>> {
    let tracks = tracks(lines);
    let mut out = Vec::new();
    let mut round = 0usize;
    let mut t = 0u32;
    let mut cur = Vec::new();
    for line in lines {
        match line["type"].as_str() {
            Some("roundStart") => {
                if t > 0 || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    round += 1;
                }
                t = 0;
            }
            Some("turn") => {
                t += 1;
                for b in line["bullets"].as_array().unwrap() {
                    if b["state"] != "MOVING" {
                        continue;
                    }
                    let (owner, id) = (
                        b["owner"].as_i64().unwrap() as u8,
                        b["id"].as_i64().unwrap(),
                    );
                    let _ = id;
                    let x = number(&b["x"]) * POS_SCALE as f64;
                    let y = number(&b["y"]) * POS_SCALE as f64;
                    // Match by owner and nearest predicted position among that owner's live bullets.
                    if let Some((i, _)) = tracks[round]
                        .bullets
                        .iter()
                        .enumerate()
                        .filter(|(_, bt)| {
                            bt.owner == owner && bt.born <= t && bt.end.is_none_or(|e| e.0 > t)
                        })
                        .map(|(i, bt)| {
                            let (px, py) = bt.position(t);
                            (i, (px - x).hypot(py - y))
                        })
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                    {
                        cur.push((i, t, x, y));
                    }
                }
            }
            _ => {}
        }
    }
    out.push(cur);
    let _ = round;
    out
}
