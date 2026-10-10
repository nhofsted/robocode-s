// SPDX-License-Identifier: EPL-2.0
//
// Ported from fdlibm 5.3 (s_scalbn.c 1.3):
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

use crate::words::{hi, lo, with_hi};

const TWO54: f64 = 1.80143985094819840000e+16;
const TWOM54: f64 = 5.55111512312578270212e-17;
const HUGE: f64 = 1.0e+300;
const TINY: f64 = 1.0e-300;

/// `scalbn(x, n)`: x * 2^n computed by exponent manipulation.
pub(crate) fn scalbn(mut x: f64, n: i32) -> f64 {
    let mut hx = hi(x);
    let lx = lo(x);
    let mut k = (hx & 0x7ff0_0000) >> 20; // extract exponent
    if k == 0 {
        // 0 or subnormal x
        if (lx | (hx & 0x7fff_ffff) as u32) == 0 {
            return x; // +-0
        }
        x *= TWO54;
        hx = hi(x);
        k = ((hx & 0x7ff0_0000) >> 20) - 54;
        if n < -50000 {
            return TINY * x; // underflow
        }
    }
    if k == 0x7ff {
        return x + x; // NaN or Inf
    }
    k += n;
    if k > 0x7fe {
        return HUGE * HUGE.copysign(x); // overflow
    }
    if k > 0 {
        // normal result
        return with_hi(x, (hx & 0x800f_ffffu32 as i32) | (k << 20));
    }
    if k <= -54 {
        return if n > 50000 {
            HUGE * HUGE.copysign(x) // overflow; in case integer overflow in n+k
        } else {
            TINY * TINY.copysign(x) // underflow
        };
    }
    k += 54; // subnormal result
    with_hi(x, (hx & 0x800f_ffffu32 as i32) | (k << 20)) * TWOM54
}
