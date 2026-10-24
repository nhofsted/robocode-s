// SPDX-License-Identifier: EPL-2.0

//! Writes the small track `web/src/lib/replay/fixtures/small.track` (the website's decoder test):
//! `cargo run -p robocode-s-replay-wasm --example fixture -- web/src/lib/replay/fixtures/small.track`.
//! Robot `i` drives east in frame `t` at x = 200 + 300 i + 4 t units, y = 300 + 100 i, body
//! 8 t, gun 256, radar 512 + 8 t, energy 100 - i; robot 2 is dead from frame 40, standing still.
//! Robot 0 fires east in frame 10 (power 2, hits robot 1 in frame 30) and north in frame 20
//! (hits a wall in frame 50); robot 1 fires west in frame 25 and the bullet is in flight at the end.

use robocode_s_replay::{BulletEnd, BulletTrack, RobotFrame, RobotState, Track, encode};

fn main() {
    let path = std::env::args().nth(1).expect("usage: fixture <out.track>");
    let frames = (0..60i64)
        .map(|t| {
            (0..3i64)
                .map(|i| {
                    let dead = i == 2 && t >= 40;
                    let t = if dead { 39 } else { t }; // the dead stand still
                    RobotFrame {
                        state: if dead {
                            RobotState::Dead
                        } else {
                            RobotState::Active
                        },
                        x: 200 + 300 * i + 4 * t,
                        y: 300 + 100 * i,
                        body: 8 * t,
                        gun: 256,
                        radar: 512 + 8 * t,
                        energy: if dead { 0 } else { (100 - i) * 100 },
                    }
                })
                .collect()
        })
        .collect();
    let bullet = |owner: u8, born: u32, heading, power, end| {
        let (t, i) = (i64::from(born), i64::from(owner));
        BulletTrack {
            owner,
            born,
            x: 200 + 300 * i + 4 * t,
            y: 300 + 100 * i,
            heading,
            power,
            end,
        }
    };
    let bullets = vec![
        bullet(0, 10, 16384, 200, Some((30, BulletEnd::Robot(1), 700, 400))),
        bullet(0, 20, 0, 100, Some((50, BulletEnd::Wall, 500, 1200))),
        bullet(1, 25, 49152, 300, None),
    ];
    let track = Track {
        engine_version: 1,
        width: 800,
        height: 600,
        gun_cooling_milli: 100,
        frames,
        bullets,
    };
    std::fs::write(path, encode(&track)).unwrap();
}
