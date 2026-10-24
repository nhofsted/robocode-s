// SPDX-License-Identifier: EPL-2.0

//! The replay-track decoder (ADR 0017, `crates/replay`) for the website: a track decoded
//! into flat float arrays in pixels, radians and energy units, so the viewer never needs
//! the quantization constants. Built by `web/tools/build-wasm.sh`.
//!
//! Headings are radians as the desktop GUI uses them: 0 = north (up, +y), clockwise.

use robocode_s_replay::{
    ANGLE_STEPS, BULLET_ANGLE_STEPS, BulletEnd, ENERGY_SCALE, POS_SCALE, RobotState, Track,
};
use std::f64::consts::PI;
use wasm_bindgen::prelude::*;

/// Floats per robot in [`DecodedTrack::robot_frames`].
const ROBOT_STRIDE: usize = 7;
/// Floats per bullet in [`DecodedTrack::bullets`].
const BULLET_STRIDE: usize = 11;

#[wasm_bindgen]
pub struct DecodedTrack {
    track: Track,
}

/// Decodes a `.track` file.
#[wasm_bindgen]
pub fn decode(bytes: &[u8]) -> Result<DecodedTrack, JsError> {
    let track = robocode_s_replay::decode(bytes)
        .map_err(|e| JsError::new(&format!("not a recording: {}", e.0)))?;
    Ok(DecodedTrack { track })
}

fn px(units: i64) -> f32 {
    (units as f64 / POS_SCALE as f64) as f32
}

fn rad(units: i64, steps: i64) -> f32 {
    (units as f64 * (2.0 * PI / steps as f64)) as f32
}

fn energy(units: i64) -> f32 {
    (units as f64 / ENERGY_SCALE as f64) as f32
}

fn state(s: RobotState) -> f32 {
    match s {
        RobotState::Active => 0.0,
        RobotState::HitWall => 1.0,
        RobotState::HitRobot => 2.0,
        RobotState::Dead => 3.0,
    }
}

#[wasm_bindgen]
impl DecodedTrack {
    #[wasm_bindgen(getter, js_name = engineVersion)]
    pub fn engine_version(&self) -> u32 {
        self.track.engine_version
    }

    /// Field width in pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> f64 {
        self.track.width as f64
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> f64 {
        self.track.height as f64
    }

    #[wasm_bindgen(getter, js_name = gunCoolingMilli)]
    pub fn gun_cooling_milli(&self) -> f64 {
        self.track.gun_cooling_milli as f64
    }

    #[wasm_bindgen(getter, js_name = robotCount)]
    pub fn robot_count(&self) -> usize {
        self.track.frames.first().map_or(0, Vec::len)
    }

    #[wasm_bindgen(getter, js_name = frameCount)]
    pub fn frame_count(&self) -> usize {
        self.track.frames.len()
    }

    #[wasm_bindgen(getter, js_name = bulletCount)]
    pub fn bullet_count(&self) -> usize {
        self.track.bullets.len()
    }

    /// `[frame][robot][7]` flat: state (0 active, 1 hit wall, 2 hit robot, 3 dead), x, y (px),
    /// body, gun, radar heading (radians), energy.
    #[wasm_bindgen(js_name = robotFrames)]
    pub fn robot_frames(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.frame_count() * self.robot_count() * ROBOT_STRIDE);
        for r in self.track.frames.iter().flatten() {
            out.extend([
                state(r.state),
                px(r.x),
                px(r.y),
                rad(r.body, ANGLE_STEPS),
                rad(r.gun, ANGLE_STEPS),
                rad(r.radar, ANGLE_STEPS),
                energy(r.energy),
            ]);
        }
        out
    }

    /// `[bullet][11]` flat, in order of birth: owner, born frame, x, y (px, at birth), heading
    /// (radians), power, end frame (-1 none), end kind (-1 none, 0 wall, 1 bullet, 2 robot),
    /// victim (robot index, -1 unless a robot), end x, end y (px, 0 if none).
    pub fn bullets(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.track.bullets.len() * BULLET_STRIDE);
        for b in &self.track.bullets {
            let (end, kind, victim, ex, ey) = match b.end {
                Some((f, k, x, y)) => {
                    let (kind, victim) = match k {
                        BulletEnd::Wall => (0.0, -1.0),
                        BulletEnd::Bullet => (1.0, -1.0),
                        BulletEnd::Robot(v) => (2.0, f32::from(v)),
                    };
                    (f as f32, kind, victim, px(x), px(y))
                }
                None => (-1.0, -1.0, -1.0, 0.0, 0.0),
            };
            out.extend([
                f32::from(b.owner),
                b.born as f32,
                px(b.x),
                px(b.y),
                rad(b.heading, BULLET_ANGLE_STEPS),
                energy(b.power),
                end,
                kind,
                victim,
                ex,
                ey,
            ]);
        }
        out
    }

    /// The bullets in flight in `frame` (born <= frame, and not ended by then), as
    /// `[index, x, y]` triples with x, y in px. A bullet ending in `frame` is not included:
    /// the renderer draws its explosion at the end position from [`Self::bullets`].
    #[wasm_bindgen(js_name = bulletPositions)]
    pub fn bullet_positions(&self, frame: u32) -> Vec<f32> {
        let mut out = Vec::new();
        for (i, b) in self.track.bullets.iter().enumerate() {
            if b.born > frame {
                break; // in order of birth
            }
            if b.end.is_some_and(|(end, ..)| end <= frame) {
                continue;
            }
            let (x, y) = b.position(frame);
            out.extend([
                i as f32,
                (x / POS_SCALE as f64) as f32,
                (y / POS_SCALE as f64) as f32,
            ]);
        }
        out
    }

    /// Per robot, the first frame it is dead in, -1 if it never dies.
    #[wasm_bindgen(js_name = deathFrames)]
    pub fn death_frames(&self) -> Vec<i32> {
        (0..self.robot_count())
            .map(|i| {
                self.track
                    .frames
                    .iter()
                    .position(|f| f[i].state == RobotState::Dead)
                    .map_or(-1, |t| t as i32)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use robocode_s_replay::{BulletTrack, RobotFrame, encode};

    fn robot(state: RobotState, x: i64, body: i64) -> RobotFrame {
        RobotFrame {
            state,
            x,
            y: 300,
            body,
            gun: 256,
            radar: 512,
            energy: 10050,
        }
    }

    fn sample() -> Vec<u8> {
        let frames = vec![
            vec![
                robot(RobotState::Active, 200, 0),
                robot(RobotState::Active, 600, 0),
            ],
            vec![
                robot(RobotState::HitWall, 202, 4),
                robot(RobotState::Dead, 600, 0),
            ],
            vec![
                robot(RobotState::Active, 204, 8),
                robot(RobotState::Dead, 600, 0),
            ],
        ];
        let bullets = vec![
            BulletTrack {
                owner: 0,
                born: 1,
                x: 202,
                y: 300,
                heading: 16384,
                power: 200,
                end: Some((2, BulletEnd::Robot(1), 230, 300)),
            },
            BulletTrack {
                owner: 1,
                born: 1,
                x: 600,
                y: 300,
                heading: 0,
                power: 100,
                end: None,
            },
        ];
        let track = Track {
            engine_version: 1,
            width: 800,
            height: 600,
            gun_cooling_milli: 100,
            frames,
            bullets,
        };
        encode(&track)
    }

    #[test]
    fn metadata_and_robot_frames() {
        let t = decode(&sample()).ok().unwrap();
        assert_eq!(
            (t.width(), t.height(), t.gun_cooling_milli()),
            (800.0, 600.0, 100.0)
        );
        assert_eq!(
            (t.robot_count(), t.frame_count(), t.bullet_count()),
            (2, 3, 2)
        );
        let f = t.robot_frames();
        assert_eq!(f.len(), 3 * 2 * ROBOT_STRIDE);
        // Frame 1, robot 0: hit wall, x = 202 / 2 px, body = 4 / 1024 turn, gun a quarter turn.
        let r = &f[ROBOT_STRIDE * 2..ROBOT_STRIDE * 3];
        assert_eq!(r[0], 1.0);
        assert_eq!((r[1], r[2]), (101.0, 150.0));
        assert!((r[3] - 4.0 * 2.0 * std::f32::consts::PI / 1024.0).abs() < 1e-6);
        assert!((r[4] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((r[5] - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(r[6], 100.5);
        assert_eq!(f[ROBOT_STRIDE * 3], 3.0);
    }

    #[test]
    fn bullets_deaths_and_positions() {
        let t = decode(&sample()).ok().unwrap();
        assert_eq!(t.death_frames(), vec![-1, 1]);
        let b = t.bullets();
        assert_eq!(b.len(), 2 * BULLET_STRIDE);
        assert_eq!(&b[..4], &[0.0, 1.0, 101.0, 150.0]);
        assert!((b[4] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(&b[5..9], &[2.0, 2.0, 2.0, 1.0]);
        assert_eq!(&b[9..11], &[115.0, 150.0]);
        assert_eq!(&b[11 + 6..11 + 11], &[-1.0, -1.0, -1.0, 0.0, 0.0]);
        // Nothing yet in frame 0; both bullets are born in frame 1, where bullet 0 ends in
        // frame 2. Bullet 1 flies north at 17 px per turn.
        assert!(t.bullet_positions(0).is_empty());
        assert_eq!(
            t.bullet_positions(1),
            vec![0.0, 101.0, 150.0, 1.0, 300.0, 150.0]
        );
        assert_eq!(t.bullet_positions(2), vec![1.0, 300.0, 167.0]);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(robocode_s_replay::decode(&[]).is_err());
    }
}
