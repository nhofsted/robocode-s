// SPDX-License-Identifier: EPL-2.0

//! Pure core of the rumble backend and client (docs/rumble-backend.md): no I/O, no async,
//! no randomness. Callers pass random bytes and the current time in.

pub mod api;
pub mod auth;
pub mod batch;
pub mod bots;
pub mod cleanup;
pub mod codesize;
pub mod config;
pub mod data;
pub mod ids;
pub mod jar;
pub mod metrics;
pub mod plan;
pub mod publish;
pub mod record;
pub mod results;
pub mod slots;
pub mod state;
pub mod store;
pub mod token;
pub mod verify;
pub mod work;

pub use config::Config;
pub use ids::{Division, RumbleId, WeightClass};
