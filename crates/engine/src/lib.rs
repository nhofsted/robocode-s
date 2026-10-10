// SPDX-License-Identifier: EPL-2.0

//! Deterministic Robocode battle engine.
//!
//! A pure simulation with no I/O, threads or clocks: each turn is
//! `step(state, intents) -> (state', events)` (design §4).
