// SPDX-License-Identifier: EPL-2.0

//! Port of fdlibm 5.3, bit-identical to `java.lang.StrictMath`.
//!
//! The engine uses these functions for every transcendental operation so that
//! results are identical on every platform (design §4.3, ADR 0003). The code
//! follows the original C closely, including its constants and operation order;
//! tests compare it with `StrictMath` from a real JDK (`tests/data/`).
//!
//! Ported from the original fdlibm 5.3 sources at netlib (the reference named
//! by the `StrictMath` specification), not from the JDK's `FdLibm.java`.

// Keep the code recognizably close to fdlibm.
#![allow(
    clippy::approx_constant,
    clippy::excessive_precision,
    clippy::unreadable_literal,
    clippy::assign_op_pattern,
    clippy::explicit_counter_loop,
    clippy::eq_op // `x - x` deliberately turns Inf or NaN into NaN
)]

mod atan;
mod hypot;
mod kernels;
mod log;
mod rem_pio2;
mod scalbn;
mod trig;
mod words;

pub use atan::{atan, atan2};
pub use hypot::hypot;
pub use log::log;
pub use trig::{cos, sin, tan};

/// `StrictMath.sqrt(x)`. IEEE 754 requires a correctly rounded result, so this is
/// the hardware square root; provided for completeness.
#[inline]
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

const DEGREES_TO_RADIANS: f64 = 0.017453292519943295;
const RADIANS_TO_DEGREES: f64 = 57.29577951308232;

/// `StrictMath.toRadians(angdeg)` as implemented since JDK 9 (a single
/// multiplication; JDK 8 computed `angdeg / 180.0 * PI`).
#[inline]
pub fn to_radians(angdeg: f64) -> f64 {
    angdeg * DEGREES_TO_RADIANS
}

/// `StrictMath.toDegrees(angrad)` as implemented since JDK 9.
#[inline]
pub fn to_degrees(angrad: f64) -> f64 {
    angrad * RADIANS_TO_DEGREES
}
