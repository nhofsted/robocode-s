// SPDX-License-Identifier: EPL-2.0

//! Java's `Math.min`, `Math.max` and `Math.signum` for doubles. They differ from Rust's
//! for NaN and signed zeros, which can matter for exact results.

/// `Math.min(a, b)`: NaN if either is NaN; -0.0 is smaller than 0.0.
pub fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return a;
    }
    if a == 0.0 && b == 0.0 && b.is_sign_negative() {
        return b;
    }
    if a <= b { a } else { b }
}

/// `Math.max(a, b)`: NaN if either is NaN; 0.0 is larger than -0.0.
pub fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        return a;
    }
    if a == 0.0 && b == 0.0 && a.is_sign_negative() {
        return b;
    }
    if a >= b { a } else { b }
}

/// `Math.signum(d)`: zero and NaN are returned unchanged (Rust's `signum(-0.0)` is -1).
pub fn signum(d: f64) -> f64 {
    if d > 0.0 {
        1.0
    } else if d < 0.0 {
        -1.0
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_java_semantics() {
        assert!(min(f64::NAN, 1.0).is_nan());
        assert!(min(1.0, f64::NAN).is_nan());
        assert!(min(0.0, -0.0).is_sign_negative());
        assert!(min(-0.0, 0.0).is_sign_negative());
        assert!(max(-0.0, 0.0).is_sign_positive());
        assert!(max(0.0, -0.0).is_sign_positive());
        assert!(max(f64::NAN, 1.0).is_nan());
        assert!(signum(-0.0).is_sign_negative() && signum(-0.0) == 0.0);
        assert_eq!(signum(-3.0), -1.0);
        assert!(signum(f64::NAN).is_nan());
    }
}
