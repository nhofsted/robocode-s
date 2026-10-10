// SPDX-License-Identifier: EPL-2.0
//
// Ported from fdlibm 5.3 (e_hypot.c 1.3):
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

use crate::words::{from_words, hi, lo, with_hi};

/// `StrictMath.hypot(x, y)`: sqrt(x*x + y*y) without undue overflow or underflow.
pub fn hypot(x: f64, y: f64) -> f64 {
    let mut ha = hi(x) & 0x7fff_ffff; // high word of x
    let mut hb = hi(y) & 0x7fff_ffff; // high word of y
    let (mut a, mut b) = if hb > ha {
        std::mem::swap(&mut ha, &mut hb);
        (y, x)
    } else {
        (x, y)
    };
    a = with_hi(a, ha); // a <- |a|
    b = with_hi(b, hb); // b <- |b|
    if (ha - hb) > 0x3c0_0000 {
        return a + b; // x/y > 2**60
    }
    let mut k = 0;
    if ha > 0x5f30_0000 {
        // a > 2**500
        if ha >= 0x7ff0_0000 {
            // Inf or NaN
            let mut w = a + b; // for sNaN
            if ((ha & 0xf_ffff) as u32 | lo(a)) == 0 {
                w = a;
            }
            if ((hb ^ 0x7ff0_0000) as u32 | lo(b)) == 0 {
                w = b;
            }
            return w;
        }
        // scale a and b by 2**-600
        ha -= 0x2580_0000;
        hb -= 0x2580_0000;
        k += 600;
        a = with_hi(a, ha);
        b = with_hi(b, hb);
    }
    if hb < 0x20b0_0000 {
        // b < 2**-500
        if hb <= 0x000f_ffff {
            // subnormal b or 0
            if (hb as u32 | lo(b)) == 0 {
                return a;
            }
            let t1 = from_words(0x7fd0_0000, 0); // t1 = 2^1022
            b *= t1;
            a *= t1;
            k -= 1022;
        } else {
            // scale a and b by 2^600
            ha += 0x2580_0000; // a *= 2^600
            hb += 0x2580_0000; // b *= 2^600
            k -= 600;
            a = with_hi(a, ha);
            b = with_hi(b, hb);
        }
    }
    // medium size a and b
    let mut w = a - b;
    if w > b {
        let t1 = from_words(ha, 0);
        let t2 = a - t1;
        w = (t1 * t1 - (b * (-b) - t2 * (a + t1))).sqrt();
    } else {
        a = a + a;
        let y1 = from_words(hb, 0);
        let y2 = b - y1;
        let t1 = from_words(ha + 0x0010_0000, 0);
        let t2 = a - t1;
        w = (t1 * y1 - (w * (-w) - (t1 * y2 + t2 * b))).sqrt();
    }
    if k != 0 {
        let t1 = with_hi(1.0, hi(1.0) + (k << 20));
        t1 * w
    } else {
        w
    }
}
