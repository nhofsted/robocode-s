// SPDX-License-Identifier: EPL-2.0

//! Classic's `robocode.Rules` and the engine constants (engine-spec §2).

/// `Math.toRadians(1)` since JDK 9.
const DEGREES_TO_RADIANS: f64 = 0.017453292519943295;

pub const ACCELERATION: f64 = 1.0;
pub const DECELERATION: f64 = 2.0;
pub const MAX_VELOCITY: f64 = 8.0;
pub const RADAR_SCAN_RADIUS: f64 = 1200.0;
pub const MIN_BULLET_POWER: f64 = 0.1;
pub const MAX_BULLET_POWER: f64 = 3.0;
pub const MAX_TURN_RATE_RADIANS: f64 = 10.0 * DEGREES_TO_RADIANS;
pub const GUN_TURN_RATE_RADIANS: f64 = 20.0 * DEGREES_TO_RADIANS;
pub const RADAR_TURN_RATE_RADIANS: f64 = 45.0 * DEGREES_TO_RADIANS;
pub const ROBOT_HIT_DAMAGE: f64 = 0.6;
pub const ROBOT_HIT_BONUS: f64 = 2.0 * ROBOT_HIT_DAMAGE;

/// Robots are 36×36 squares.
pub const ROBOT_SIZE: i32 = 36;
pub const ROBOT_HALF_SIZE: i32 = ROBOT_SIZE / 2;
/// Bullets hit walls within this distance.
pub const BULLET_RADIUS: f64 = 3.0;
pub const BULLET_EXPLOSION_LENGTH: i32 = 17;
pub const ROBOT_EXPLOSION_LENGTH: i32 = 71;
/// `BaseBattle.TURNS_DISPLAYED_AFTER_ENDING`.
pub const TURNS_DISPLAYED_AFTER_ENDING: i32 = 30;

pub fn bullet_damage(power: f64) -> f64 {
    let mut damage = 4.0 * power;
    if power > 1.0 {
        damage += 2.0 * (power - 1.0);
    }
    damage
}

pub fn bullet_hit_bonus(power: f64) -> f64 {
    3.0 * power
}

pub fn bullet_speed(power: f64) -> f64 {
    let power = crate::java::min(crate::java::max(power, MIN_BULLET_POWER), MAX_BULLET_POWER);
    20.0 - 3.0 * power
}

pub fn gun_heat(power: f64) -> f64 {
    1.0 + (power / 5.0)
}

pub fn wall_hit_damage(velocity: f64) -> f64 {
    crate::java::max(velocity.abs() / 2.0 - 1.0, 0.0)
}
