// SPDX-License-Identifier: EPL-2.0

//! A robot's command state (classic's `ExecCommands`, engine-spec §11.1).

use crate::rules::{MAX_TURN_RATE_RADIANS, MAX_VELOCITY};

#[derive(Debug, Clone, PartialEq)]
pub struct BulletCommand {
    pub id: i32,
    pub power: f64,
    /// Fire assist (basic robots only): aim at this angle if the radar turned with the gun.
    pub assist_valid: bool,
    pub assist_angle: f64,
    pub consumed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeamMessage {
    pub sender: String,
    /// `None` sends to all teammates.
    pub recipient: Option<String>,
    pub bytes: Vec<u8>,
}

/// The state a robot sends with every `execute`; the engine updates the remaining values
/// while executing and reports them back.
#[derive(Debug, Clone, PartialEq)]
pub struct Commands {
    pub body_turn_remaining: f64,
    pub gun_turn_remaining: f64,
    pub radar_turn_remaining: f64,
    pub distance_remaining: f64,
    pub adjust_gun_for_body_turn: bool,
    pub adjust_radar_for_gun_turn: bool,
    pub adjust_radar_for_body_turn: bool,
    pub max_turn_rate: f64,
    pub max_velocity: f64,
    pub scan: bool,
    /// The robot did file I/O (allows more skipped turns).
    pub io: bool,
    pub bullets: Vec<BulletCommand>,
    pub messages: Vec<TeamMessage>,
}

impl Default for Commands {
    fn default() -> Self {
        Self {
            body_turn_remaining: 0.0,
            gun_turn_remaining: 0.0,
            radar_turn_remaining: 0.0,
            distance_remaining: 0.0,
            adjust_gun_for_body_turn: false,
            adjust_radar_for_gun_turn: false,
            adjust_radar_for_body_turn: false,
            max_turn_rate: f64::MAX,
            max_velocity: f64::MAX,
            scan: false,
            io: false,
            bullets: Vec::new(),
            messages: Vec::new(),
        }
    }
}

impl Commands {
    /// Validation when commands arrive (`RobotPeer.validateCommands`): the limits are
    /// capped to the rules. NaN stays NaN.
    pub fn validate(&mut self) {
        self.max_turn_rate = crate::java::min(self.max_turn_rate.abs(), MAX_TURN_RATE_RADIANS);
        self.max_velocity = crate::java::min(self.max_velocity.abs(), MAX_VELOCITY);
    }
}
