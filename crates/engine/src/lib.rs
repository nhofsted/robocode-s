// SPDX-License-Identifier: EPL-2.0

//! Deterministic Robocode battle engine, bit-exact with classic Robocode 1.11.1
//! (docs/engine-spec.md, ADR 0003).
//!
//! A pure simulation with no I/O, threads or clocks. Robots' decisions come in as
//! [`Commands`]; each [`Battle::step`] runs one turn and produces [`Event`]s.

/// Version of the engine as far as battle outcomes go (design §4.4). Bump it with every
/// change that can alter a battle's outcome: rules and physics, the PRNG, fuel counting and
/// its calibration (including the Java-side constants in `clients/java-determinism`, so every
/// `DETERMINISTIC_RULES` bump), java-compat behaviour, the pinned JRE. Separate from the
/// protocol version and the release version.
pub const ENGINE_VERSION: u32 = 1;

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
