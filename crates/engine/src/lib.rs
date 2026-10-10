// SPDX-License-Identifier: EPL-2.0

//! Deterministic Robocode battle engine, bit-exact with classic Robocode 1.11.1
//! (docs/engine-spec.md, ADR 0003).
//!
//! A pure simulation with no I/O, threads or clocks. Robots' decisions come in as
//! [`Commands`]; each [`Battle::step`] runs one turn and produces [`Event`]s.

pub mod battle;
pub mod commands;
pub mod events;
mod java;
pub mod rules;
pub mod stats;
pub mod utils;

pub use battle::{
    Battle, BattleRules, Bullet, BulletKind, BulletState, BulletStatus, Robot, RobotSetup,
    RobotState, RobotStatus,
};
pub use commands::{BulletCommand, Commands, TeamMessage};
pub use events::{BulletInfo, Event, Results};
pub use robocode_s_java_random as java_random;
pub use stats::Statistics;
