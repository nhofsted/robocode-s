// SPDX-License-Identifier: EPL-2.0

//! A round's lossy viewer track (ADR 0017, docs/rumble-backend.md §13.4), built from the
//! snapshots the battle's observer saw: positions in ½ px, headings in 1/1024 of a turn,
//! energy in 0.01, bullets from their first and last snapshot state. The quantization is
//! `crates/replay`'s, and its codec's float math is fdlibm's, so every volunteer gets the
//! same bytes for the same round.

use robocode_s_engine::ENGINE_VERSION;
use robocode_s_protocol::v1;
use robocode_s_replay::{
    ANGLE_STEPS, BULLET_ANGLE_STEPS, BulletEnd, BulletTrack, ENERGY_SCALE, POS_SCALE, RobotFrame,
    RobotState, Track, angle_units,
};
use rumble_core::api::Rules;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// An encoded track and its SHA-256, as the results post claims it (`track {sha256, size}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub sha256: [u8; 32],
}

impl Encoded {
    pub fn new(track: &Track) -> Self {
        let bytes = robocode_s_replay::encode(track);
        let sha256 = Sha256::digest(&bytes).into();
        Self { bytes, sha256 }
    }
}

fn pos(v: Option<f64>) -> i64 {
    (v.unwrap_or(0.0) * POS_SCALE as f64).round() as i64
}

fn energy(v: Option<f64>) -> i64 {
    (v.unwrap_or(0.0) * ENERGY_SCALE as f64).round() as i64
}

fn state(s: i32) -> RobotState {
    match v1::RobotState::try_from(s) {
        Ok(v1::RobotState::HitWall) => RobotState::HitWall,
        Ok(v1::RobotState::HitRobot) => RobotState::HitRobot,
        Ok(v1::RobotState::Dead) => RobotState::Dead,
        _ => RobotState::Active,
    }
}

/// One round's track from its snapshots: `snapshots[0]` is the round start (turn 0), then
/// one per turn, as the server broadcasts them.
pub fn from_snapshots(rules: &Rules, snapshots: &[v1::TurnSnapshot]) -> Track {
    let mut track = Track {
        engine_version: ENGINE_VERSION,
        width: i64::from(rules.width),
        height: i64::from(rules.height),
        gun_cooling_milli: (rules.gun_cooling_rate * 1000.0).round() as i64,
        frames: Vec::with_capacity(snapshots.len()),
        bullets: Vec::new(),
    };
    // (owner, bullet id) → index in `track.bullets`.
    let mut ids: HashMap<(i32, i32), usize> = HashMap::new();
    for (t, snapshot) in snapshots.iter().enumerate() {
        let t = t as u32;
        let frame: Vec<RobotFrame> = snapshot
            .robots
            .iter()
            .enumerate()
            .map(|(i, r)| {
                // The codec keeps a dead robot as it died, so the track does too: then it
                // equals what decodes from it (#104). The bytes are the same either way.
                if let Some(last) = track.frames.last().and_then(|f: &Vec<RobotFrame>| f.get(i))
                    && last.state == RobotState::Dead
                {
                    return *last;
                }
                RobotFrame {
                    state: state(r.state),
                    x: pos(r.x),
                    y: pos(r.y),
                    body: angle_units(r.body_heading.unwrap_or(0.0), ANGLE_STEPS),
                    gun: angle_units(r.gun_heading.unwrap_or(0.0), ANGLE_STEPS),
                    radar: angle_units(r.radar_heading.unwrap_or(0.0), ANGLE_STEPS),
                    energy: energy(r.energy),
                }
            })
            .collect();
        track.frames.push(frame);
        if t == 0 {
            continue; // the codec has no bullets at the round start
        }
        for b in &snapshot.bullets {
            let kind = match v1::BulletState::try_from(b.state) {
                Ok(v1::BulletState::Fired | v1::BulletState::Moving) => None,
                Ok(v1::BulletState::HitWall) => Some(BulletEnd::Wall),
                Ok(v1::BulletState::HitBullet) => Some(BulletEnd::Bullet),
                Ok(v1::BulletState::HitVictim) => Some(BulletEnd::Robot(b.victim.max(0) as u8)),
                // A robot's death explosion is implied by its death; inactive bullets are gone.
                _ => continue,
            };
            if b.explosion {
                continue;
            }
            let (x, y) = (pos(b.x), pos(b.y));
            let index = *ids.entry((b.owner, b.bullet_id)).or_insert_with(|| {
                track.bullets.push(BulletTrack {
                    owner: b.owner as u8,
                    born: t,
                    x,
                    y,
                    heading: angle_units(b.heading.unwrap_or(0.0), BULLET_ANGLE_STEPS),
                    power: energy(b.power),
                    end: None,
                });
                track.bullets.len() - 1
            });
            let bullet = &mut track.bullets[index];
            if let Some(kind) = kind
                && bullet.end.is_none()
            {
                bullet.end = Some((t, kind, x, y));
            }
        }
    }
    // The codec stores births per robot in robot order within a frame (stable: a robot's
    // bullets of one turn keep their order).
    track.bullets.sort_by_key(|b| (b.born, b.owner));
    track
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            rounds: 1,
            width: 800,
            height: 600,
            gun_cooling_rate: 0.1,
            inactivity_time: 450,
            sentry_border: 100,
            hide_enemy_names: false,
        }
    }

    fn robot(x: f64, state: v1::RobotState, energy: f64) -> v1::RobotSnapshot {
        v1::RobotSnapshot {
            state: state as i32,
            x: Some(x),
            y: Some(300.0),
            energy: Some(energy),
            body_heading: Some(1.0),
            gun_heading: Some(2.0),
            radar_heading: Some(3.0),
            ..Default::default()
        }
    }

    fn bullet(id: i32, state: v1::BulletState, x: f64) -> v1::BulletSnapshot {
        v1::BulletSnapshot {
            owner: 0,
            bullet_id: id,
            state: state as i32,
            x: Some(x),
            y: Some(300.0),
            heading: Some(std::f64::consts::FRAC_PI_2),
            power: Some(1.5),
            victim: if state == v1::BulletState::HitVictim {
                1
            } else {
                -1
            },
            ..Default::default()
        }
    }

    #[test]
    fn bullets_are_born_and_end_once_and_the_track_round_trips() {
        use v1::BulletState::*;
        use v1::RobotState::*;
        let snaps: Vec<v1::TurnSnapshot> = (0..6)
            .map(|t| v1::TurnSnapshot {
                round: 0,
                turn: t,
                robots: vec![
                    robot(100.0 + t as f64, Active, 100.0),
                    robot(
                        200.0,
                        if t >= 4 { Dead } else { Active },
                        if t >= 4 { 0.0 } else { 90.0 },
                    ),
                ],
                bullets: match t {
                    1 => vec![bullet(1, Fired, 110.0)],
                    2 => vec![bullet(1, Moving, 135.5)],
                    3 => vec![bullet(1, HitVictim, 160.25)],
                    4 => vec![bullet(1, HitVictim, 160.25)],
                    _ => vec![],
                },
            })
            .collect();
        let track = from_snapshots(&rules(), &snaps);
        assert_eq!(track.frames.len(), 6);
        assert_eq!(track.engine_version, ENGINE_VERSION);
        assert_eq!(track.bullets.len(), 1);
        let b = &track.bullets[0];
        assert_eq!((b.born, b.x, b.power), (1, 220, 150));
        assert_eq!(b.end, Some((3, BulletEnd::Robot(1), 321, 600)));
        assert_eq!(track.frames[5][1], track.frames[4][1]);
        let encoded = Encoded::new(&track);
        assert_eq!(robocode_s_replay::decode(&encoded.bytes).unwrap(), track);
        assert_eq!(Encoded::new(&track), encoded, "deterministic");
    }
}
