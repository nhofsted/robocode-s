// SPDX-License-Identifier: EPL-2.0

//! The website's lossy viewer tracks (ADR 0017, `crates/replay`) as the battle screen's
//! snapshots. A track is one round as the viewer draws it: positions, headings, energy and
//! bullets, without names, scores, debug paint or console output. It is decoded once into
//! a snapshot per turn, so it plays without the engine.
//!
//! A track downloaded from the website has its battle appended
//! ([`robocode_s_replay::with_battle`], docs/rumble-data.md): the robots, the seed and the
//! round, which name the robots and let the battle run again ([`crate::rerun`]).

use crate::sprites::hsl;
use robocode_s_engine::rules::{BULLET_EXPLOSION_LENGTH, ROBOT_EXPLOSION_LENGTH};
use robocode_s_protocol::v1;
use robocode_s_replay::{
    ANGLE_STEPS, BULLET_ANGLE_STEPS, BulletEnd, ENERGY_SCALE, POS_SCALE, RobotState, Track,
};
use serde::Deserialize;
use std::f64::consts::PI;

/// A decoded track: the field size, a name per robot (a placeholder without its battle)
/// and every turn.
pub struct Decoded {
    pub engine_version: u32,
    pub width: f64,
    pub height: f64,
    /// Gun cooling rate in thousandths.
    pub gun_cooling_milli: i64,
    pub robots: Vec<v1::RobotStatics>,
    pub snapshots: Vec<v1::TurnSnapshot>,
    /// The rumble battle it is a round of, when the website appended it.
    pub battle: Option<TrackBattle>,
}

/// The rumble battle a track is a round of, as the website appends it to a download
/// (docs/rumble-data.md, "Downloaded tracks"). Rules it leaves out are the rumble's.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackBattle {
    pub schema: u32,
    /// The engine version the battle ran with.
    pub engine: u32,
    /// The battle's seed, a decimal string (64 bits).
    pub seed: String,
    /// 0-based round of the battle that the track is.
    pub round: i32,
    pub rounds: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub gun_cooling_rate: Option<f64>,
    #[serde(default)]
    pub inactivity_time: Option<i64>,
    #[serde(default)]
    pub sentry_border: Option<i32>,
    #[serde(default)]
    pub hide_enemy_names: Option<bool>,
    /// In battle order: the track's robot `i` is `robots[i]`.
    pub robots: Vec<TrackRobot>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TrackRobot {
    pub bot: String,
    pub version: String,
    /// The first 16 hex digits of the jar's SHA-256.
    pub sha16: String,
}

impl TrackRobot {
    /// The name classic lists it by.
    pub fn name(&self) -> String {
        format!("{} {}", self.bot, self.version)
    }
}

/// The battle appended to a track, if there is one this version reads that fits the track.
fn battle(bytes: &[u8], robots: usize) -> Option<TrackBattle> {
    let json = robocode_s_replay::split_battle(bytes).1?;
    let battle: TrackBattle = serde_json::from_slice(json).ok()?;
    (battle.schema == 1 && battle.robots.len() == robots && battle.seed.parse::<i64>().is_ok())
        .then_some(battle)
}

/// Whether `bytes` start like a track rather than a full recording (gzip, `1f 8b`). A track
/// starts with its format version (1); `crates/replay` keeps versions clear of gzip's byte.
pub fn is_track(bytes: &[u8]) -> bool {
    !bytes.starts_with(&[0x1f, 0x8b])
}

pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let track =
        robocode_s_replay::decode(bytes).map_err(|e| format!("not a recording: {}", e.0))?;
    let mut decoded = snapshots(&track);
    decoded.battle = battle(bytes, decoded.robots.len());
    if let Some(b) = &decoded.battle {
        for (statics, robot) in decoded.robots.iter_mut().zip(&b.robots) {
            let class = robot.bot.rsplit('.').next().unwrap_or(&robot.bot);
            statics.name = robot.name();
            statics.short_name = format!("{class} {}", robot.version);
            statics.full_class_name.clone_from(&robot.bot);
        }
    }
    Ok(decoded)
}

fn position(units: f64) -> Option<f64> {
    Some(units / POS_SCALE as f64)
}

fn angle(units: i64, steps: i64) -> Option<f64> {
    Some(units as f64 * (2.0 * PI / steps as f64))
}

/// A track has no colours: robot `i` gets its own hue, a golden angle from the last.
fn colors(i: usize) -> v1::Colors {
    let hue = (i as f64 * 137.508 + 120.0) % 360.0;
    let argb = |[r, g, b]: [u8; 3]| {
        (0xff00_0000u32 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)) as i32
    };
    v1::Colors {
        body: argb(hsl(hue, 0.55, 0.38)),
        gun: argb(hsl(hue, 0.35, 0.55)),
        radar: argb(hsl((hue + 180.0) % 360.0, 0.7, 0.55)),
        ..Default::default()
    }
}

fn snapshots(track: &Track) -> Decoded {
    let count = track.frames.first().map_or(0, Vec::len);
    let robots = (0..count)
        .map(|i| v1::RobotStatics {
            name: format!("Robot {}", i + 1),
            robot_index: i as i32,
            team_index: -1,
            ..Default::default()
        })
        .collect();
    let colors: Vec<v1::Colors> = (0..count).map(colors).collect();
    // When each robot died, for its explosion.
    let mut deaths = vec![None; count];
    let mut snapshots = Vec::with_capacity(track.frames.len());
    for (t, frame) in track.frames.iter().enumerate() {
        let t32 = t as u32;
        let robots: Vec<v1::RobotSnapshot> = frame
            .iter()
            .zip(&colors)
            .map(|(r, &colors)| v1::RobotSnapshot {
                state: match r.state {
                    RobotState::Active => v1::RobotState::Active,
                    RobotState::HitWall => v1::RobotState::HitWall,
                    RobotState::HitRobot => v1::RobotState::HitRobot,
                    RobotState::Dead => v1::RobotState::Dead,
                } as i32,
                x: position(r.x as f64),
                y: position(r.y as f64),
                energy: Some(r.energy as f64 / ENERGY_SCALE as f64),
                body_heading: angle(r.body, ANGLE_STEPS),
                gun_heading: angle(r.gun, ANGLE_STEPS),
                radar_heading: angle(r.radar, ANGLE_STEPS),
                colors: Some(colors),
                ..Default::default()
            })
            .collect();
        let mut bullets = Vec::new();
        for (id, b) in track.bullets.iter().enumerate() {
            if b.born > t32 {
                break; // in order of birth
            }
            let mut snapshot = v1::BulletSnapshot {
                owner: i32::from(b.owner),
                bullet_id: id as i32,
                heading: angle(b.heading, BULLET_ANGLE_STEPS),
                power: Some(b.power as f64 / ENERGY_SCALE as f64),
                victim: -1,
                ..Default::default()
            };
            match b.end {
                Some((end, kind, x, y)) if end <= t32 => {
                    let frame = (t32 - end) as i32;
                    if frame >= BULLET_EXPLOSION_LENGTH {
                        continue;
                    }
                    snapshot.state = match kind {
                        BulletEnd::Wall => v1::BulletState::HitWall,
                        BulletEnd::Bullet => v1::BulletState::HitBullet,
                        BulletEnd::Robot(victim) => {
                            snapshot.victim = i32::from(victim);
                            v1::BulletState::HitVictim
                        }
                    } as i32;
                    snapshot.frame = frame;
                    snapshot.x = position(x as f64);
                    snapshot.y = position(y as f64);
                }
                _ => {
                    let (x, y) = b.position(t32);
                    snapshot.state = if b.born == t32 {
                        v1::BulletState::Fired
                    } else {
                        v1::BulletState::Moving
                    } as i32;
                    snapshot.frame = (t32 - b.born) as i32;
                    snapshot.x = position(x);
                    snapshot.y = position(y);
                }
            }
            bullets.push(snapshot);
        }
        // A robot's death explosion is implied by its death.
        for (i, r) in robots.iter().enumerate() {
            let dead = r.state == v1::RobotState::Dead as i32;
            if !dead {
                deaths[i] = None;
                continue;
            }
            let died = *deaths[i].get_or_insert(t);
            let frame = (t - died) as i32;
            if frame < ROBOT_EXPLOSION_LENGTH {
                bullets.push(v1::BulletSnapshot {
                    owner: i as i32,
                    state: v1::BulletState::Exploded as i32,
                    x: r.x,
                    y: r.y,
                    power: Some(1.0),
                    victim: i as i32,
                    explosion: true,
                    frame,
                    ..Default::default()
                });
            }
        }
        snapshots.push(v1::TurnSnapshot {
            round: 0,
            turn: t as i64,
            robots,
            bullets,
        });
    }
    Decoded {
        engine_version: track.engine_version,
        width: track.width as f64,
        height: track.height as f64,
        gun_cooling_milli: track.gun_cooling_milli,
        robots,
        snapshots,
        battle: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use robocode_s_replay::{BulletTrack, RobotFrame};

    fn track() -> Track {
        let robot = |x, state| RobotFrame {
            state,
            x,
            y: 600,
            body: 256,
            energy: 10_000,
            ..Default::default()
        };
        Track {
            engine_version: 1,
            width: 800,
            height: 600,
            gun_cooling_milli: 100,
            frames: (0..30)
                .map(|t| {
                    let dead = if t >= 20 {
                        RobotState::Dead
                    } else {
                        RobotState::Active
                    };
                    vec![robot(200, RobotState::Active), robot(1200, dead)]
                })
                .collect(),
            bullets: vec![BulletTrack {
                owner: 0,
                born: 2,
                x: 200,
                y: 600,
                heading: BULLET_ANGLE_STEPS / 4,
                power: 300,
                end: Some((10, BulletEnd::Robot(1), 1180, 600)),
            }],
        }
    }

    #[test]
    fn a_track_decodes_to_snapshots_the_view_draws() {
        let bytes = robocode_s_replay::encode(&track());
        assert!(is_track(&bytes));
        let d = decode(&bytes).unwrap();
        assert_eq!((d.width, d.height, d.robots.len()), (800.0, 600.0, 2));
        assert_eq!(d.snapshots.len(), 30);
        let r = &d.snapshots[0].robots[0];
        assert_eq!(
            (r.x, r.y, r.energy),
            (Some(100.0), Some(300.0), Some(100.0))
        );
        assert!((r.body_heading.unwrap() - PI / 2.0).abs() < 1e-9);
        // The bullet: fired, moving east at 11 px a turn, then exploding on robot 2.
        assert!(d.snapshots[1].bullets.is_empty());
        let fired = &d.snapshots[2].bullets[0];
        assert_eq!(fired.state, v1::BulletState::Fired as i32);
        let moving = &d.snapshots[3].bullets[0];
        assert_eq!(moving.state, v1::BulletState::Moving as i32);
        assert!((moving.x.unwrap() - 111.0).abs() < 1e-6);
        let hit = &d.snapshots[10].bullets[0];
        assert_eq!(
            (hit.state, hit.victim, hit.x),
            (v1::BulletState::HitVictim as i32, 1, Some(590.0))
        );
        let after = &d.snapshots[10 + BULLET_EXPLOSION_LENGTH as usize].bullets;
        assert!(after.iter().all(|b| b.explosion));
        // Robot 2's death explosion.
        let boom = d.snapshots[21]
            .bullets
            .iter()
            .find(|b| b.explosion)
            .unwrap();
        assert_eq!((boom.frame, boom.owner, boom.x), (1, 1, Some(600.0)));
    }

    #[test]
    fn a_downloaded_track_names_its_robots_and_battle() {
        let track = robocode_s_replay::encode(&track());
        let json = br#"{"schema":1,"engine":1,"seed":"-42","round":3,"rounds":35,
            "width":800,"height":600,"rumble":"1v1-general",
            "robots":[{"bot":"jk.mega.DrussGT","version":"3.1.7","sha16":"0123456789abcdef"},
                      {"bot":"voidious.Diamond","version":"1.8.28","sha16":"fedcba9876543210"}]}"#;
        let d = decode(&robocode_s_replay::with_battle(&track, json)).unwrap();
        let names: Vec<_> = d.robots.iter().map(|r| r.short_name.as_str()).collect();
        assert_eq!(names, ["DrussGT 3.1.7", "Diamond 1.8.28"]);
        let battle = d.battle.unwrap();
        assert_eq!((battle.round, battle.seed.as_str()), (3, "-42"));
        assert_eq!(battle.robots[1].name(), "voidious.Diamond 1.8.28");
        assert_eq!(battle.gun_cooling_rate, None);
        // One robot for a track of two: not this track's battle.
        let one = br#"{"schema":1,"engine":1,"seed":"1","round":0,"rounds":35,"width":800,
            "height":600,"robots":[{"bot":"a.A","version":"1","sha16":"0123456789abcdef"}]}"#;
        let d = decode(&robocode_s_replay::with_battle(&track, one)).unwrap();
        assert!(d.battle.is_none());
        assert_eq!(d.robots[0].name, "Robot 1");
        assert!(decode(&track).unwrap().battle.is_none());
    }

    #[test]
    fn gzip_is_a_full_recording() {
        assert!(!is_track(&[0x1f, 0x8b, 8, 0]));
        assert!(decode(b"junk").is_err());
    }
}
