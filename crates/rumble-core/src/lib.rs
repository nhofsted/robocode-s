// SPDX-License-Identifier: EPL-2.0

//! Pure core of the rumble backend and client (docs/rumble-backend.md): no I/O, no async,
//! no randomness. Callers pass random bytes and the current time in.

pub mod api;
pub mod config;
pub mod ids;
pub mod token;

pub use config::Config;
pub use ids::{Division, RumbleId, WeightClass};
