// SPDX-License-Identifier: EPL-2.0

//! Access to the 32-bit halves of a double, like fdlibm's `__HI` and `__LO`.
//! The high word is signed (C `int`), the low word unsigned.

#[inline]
pub(crate) fn hi(x: f64) -> i32 {
    (x.to_bits() >> 32) as i32
}

#[inline]
pub(crate) fn lo(x: f64) -> u32 {
    x.to_bits() as u32
}

#[inline]
pub(crate) fn from_words(hi: i32, lo: u32) -> f64 {
    f64::from_bits((u64::from(hi as u32) << 32) | u64::from(lo))
}

/// `__HI(x) = hi`.
#[inline]
pub(crate) fn with_hi(x: f64, hi: i32) -> f64 {
    from_words(hi, lo(x))
}

/// `__LO(x) = lo`.
#[inline]
pub(crate) fn with_lo(x: f64, lo: u32) -> f64 {
    from_words(hi(x), lo)
}
