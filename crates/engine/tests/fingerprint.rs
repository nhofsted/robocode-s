// SPDX-License-Identifier: EPL-2.0

//! Guards `ENGINE_VERSION` (design §4.4): a small seeded battle between scripted robots, with
//! a hash over every turn's robot states and events and over the final results. The engine is
//! deterministic and its math is fdlibm, so the hash is the same on every platform.

use robocode_s_engine::{Battle, BattleRules, BulletCommand, Commands, ENGINE_VERSION, RobotSetup};

/// What the hash was when `ENGINE_VERSION` was last bumped.
const FINGERPRINT: u64 = 16843308543051080932;

fn setup(name: &str) -> RobotSetup {
    RobotSetup {
        name: name.to_string(),
        full_class_name: name.to_string(),
        advanced: false,
        team_robot: false,
        droid: false,
        sentry: false,
        leader: false,
        team: None,
    }
}

fn fnv(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash = (*hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
}

/// A small linear congruential generator: the robots' "decisions", without any robot.
struct Script(u64);

impl Script {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

fn fingerprint() -> u64 {
    let rules = BattleRules {
        num_rounds: 3,
        ..BattleRules::default()
    };
    let setups = ["test.A", "test.B", "test.C"].map(setup).to_vec();
    let mut battle = Battle::new(rules, setups, 20261019);
    let mut script = Script(1);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut bullet_ids = [0i32; 3];
    while battle.round < 3 {
        battle.start_round();
        while !battle.is_round_over() {
            for (i, bullet_id) in bullet_ids.iter_mut().enumerate() {
                if !battle.robots[i].is_alive() {
                    continue;
                }
                let mut commands = Commands {
                    body_turn_remaining: script.next(629) as f64 / 100.0 - 3.0,
                    gun_turn_remaining: script.next(629) as f64 / 100.0 - 3.0,
                    radar_turn_remaining: f64::INFINITY,
                    distance_remaining: script.next(300) as f64 - 100.0,
                    max_velocity: 4.0 + script.next(5) as f64,
                    scan: true,
                    ..Commands::default()
                };
                if script.next(6) == 0 {
                    *bullet_id += 1;
                    commands.bullets.push(BulletCommand {
                        id: *bullet_id,
                        power: 0.1 + script.next(30) as f64 / 10.0,
                        assist_valid: false,
                        assist_angle: 0.0,
                        consumed: false,
                    });
                }
                battle.set_commands(i, commands);
            }
            battle.step();
            for r in &battle.robots {
                for v in [
                    r.x,
                    r.y,
                    r.body_heading,
                    r.gun_heading,
                    r.radar_heading,
                    r.energy,
                    r.velocity,
                    r.gun_heat,
                ] {
                    fnv(&mut hash, &v.to_bits().to_le_bytes());
                }
            }
            for i in 0..3 {
                fnv(&mut hash, format!("{:?}", battle.take_events(i)).as_bytes());
                battle.take_bullet_updates(i);
            }
        }
        battle.finish_round();
    }
    fnv(&mut hash, format!("{:?}", battle.results()).as_bytes());
    hash
}

#[test]
fn battle_outcomes_match_the_engine_version() {
    assert_eq!(
        fingerprint(),
        FINGERPRINT,
        "Battle outcomes changed. If that is intended, bump ENGINE_VERSION (now {ENGINE_VERSION}) \
         in crates/engine/src/lib.rs and update FINGERPRINT here to the new hash (left)."
    );
}
