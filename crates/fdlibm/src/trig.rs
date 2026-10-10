// SPDX-License-Identifier: EPL-2.0
//
// Ported from fdlibm 5.3 (s_sin.c 1.3, s_cos.c 1.3, s_tan.c 1.3):
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

use crate::kernels::{k_cos, k_sin, k_tan};
use crate::rem_pio2::rem_pio2;
use crate::words::hi;

/// `StrictMath.sin(x)`.
pub fn sin(x: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix <= 0x3fe9_21fb {
        // |x| ~< pi/4
        return k_sin(x, 0.0, 0);
    }
    if ix >= 0x7ff0_0000 {
        return x - x; // sin(Inf or NaN) is NaN
    }
    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => k_sin(y0, y1, 1),
        1 => k_cos(y0, y1),
        2 => -k_sin(y0, y1, 1),
        _ => -k_cos(y0, y1),
    }
}

/// `StrictMath.cos(x)`.
pub fn cos(x: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix <= 0x3fe9_21fb {
        return k_cos(x, 0.0);
    }
    if ix >= 0x7ff0_0000 {
        return x - x; // cos(Inf or NaN) is NaN
    }
    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => k_cos(y0, y1),
        1 => -k_sin(y0, y1, 1),
        2 => -k_cos(y0, y1),
        _ => k_sin(y0, y1, 1),
    }
}

/// `StrictMath.tan(x)`.
pub fn tan(x: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix <= 0x3fe9_21fb {
        return k_tan(x, 0.0, 1);
    }
    if ix >= 0x7ff0_0000 {
        return x - x; // tan(Inf or NaN) is NaN
    }
    let (n, y0, y1) = rem_pio2(x);
    k_tan(y0, y1, 1 - ((n & 1) << 1)) // 1 if n even, -1 if n odd
}
