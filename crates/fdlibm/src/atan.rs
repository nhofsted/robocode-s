// SPDX-License-Identifier: EPL-2.0
//
// Ported from fdlibm 5.3 (s_atan.c 1.3, e_atan2.c 1.3):
//
// ====================================================
// Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//
// Developed at SunSoft, a Sun Microsystems, Inc. business.
// Permission to use, copy, modify, and distribute this
// software is freely granted, provided that this notice
// is preserved.
// ====================================================

use crate::words::{hi, lo};

const ATANHI: [f64; 4] = [
    4.63647609000806093515e-01, // atan(0.5)hi
    7.85398163397448278999e-01, // atan(1.0)hi
    9.82793723247329054082e-01, // atan(1.5)hi
    1.57079632679489655800e+00, // atan(inf)hi
];
const ATANLO: [f64; 4] = [
    2.26987774529616870924e-17, // atan(0.5)lo
    3.06161699786838301793e-17, // atan(1.0)lo
    1.39033110312309984516e-17, // atan(1.5)lo
    6.12323399573676603587e-17, // atan(inf)lo
];
const AT: [f64; 11] = [
    3.33333333333329318027e-01,
    -1.99999999998764832476e-01,
    1.42857142725034663711e-01,
    -1.11111104054623557880e-01,
    9.09088713343650656196e-02,
    -7.69187620504482999495e-02,
    6.66107313738753120669e-02,
    -5.83357013379057348645e-02,
    4.97687799461593236017e-02,
    -3.65315727442169155270e-02,
    1.62858201153657823623e-02,
];
const ONE: f64 = 1.0;
const HUGE: f64 = 1.0e300;

/// `StrictMath.atan(x)`.
pub fn atan(mut x: f64) -> f64 {
    let hx = hi(x);
    let ix = hx & 0x7fff_ffff;
    let id: i32;
    if ix >= 0x4410_0000 {
        // |x| >= 2^66
        if ix > 0x7ff0_0000 || (ix == 0x7ff0_0000 && lo(x) != 0) {
            return x + x; // NaN
        }
        return if hx > 0 {
            ATANHI[3] + ATANLO[3]
        } else {
            -ATANHI[3] - ATANLO[3]
        };
    }
    if ix < 0x3fdc_0000 {
        // |x| < 0.4375
        if ix < 0x3e20_0000 && HUGE + x > ONE {
            return x; // |x| < 2^-29; raise inexact
        }
        id = -1;
    } else {
        x = x.abs();
        if ix < 0x3ff3_0000 {
            // |x| < 1.1875
            if ix < 0x3fe6_0000 {
                // 7/16 <= |x| < 11/16
                id = 0;
                x = (2.0 * x - ONE) / (2.0 + x);
            } else {
                // 11/16 <= |x| < 19/16
                id = 1;
                x = (x - ONE) / (x + ONE);
            }
        } else if ix < 0x4003_8000 {
            // |x| < 2.4375
            id = 2;
            x = (x - 1.5) / (ONE + 1.5 * x);
        } else {
            // 2.4375 <= |x| < 2^66
            id = 3;
            x = -1.0 / x;
        }
    }
    // end of argument reduction
    let z = x * x;
    let w = z * z;
    // break sum from i=0 to 10 aT[i]z**(i+1) into odd and even poly
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    if id < 0 {
        x - x * (s1 + s2)
    } else {
        let id = id as usize;
        let z = ATANHI[id] - ((x * (s1 + s2) - ATANLO[id]) - x);
        if hx < 0 { -z } else { z }
    }
}

const TINY: f64 = 1.0e-300;
const ZERO: f64 = 0.0;
const PI_O_4: f64 = 7.8539816339744827900E-01;
const PI_O_2: f64 = 1.5707963267948965580E+00;
const PI: f64 = 3.1415926535897931160E+00;
const PI_LO: f64 = 1.2246467991473531772E-16;

/// `StrictMath.atan2(y, x)`.
pub fn atan2(y: f64, x: f64) -> f64 {
    let hx = hi(x);
    let ix = hx & 0x7fff_ffff;
    let lx = lo(x);
    let hy = hi(y);
    let iy = hy & 0x7fff_ffff;
    let ly = lo(y);
    if ((ix as u32 | ((lx | lx.wrapping_neg()) >> 31)) > 0x7ff0_0000)
        || ((iy as u32 | ((ly | ly.wrapping_neg()) >> 31)) > 0x7ff0_0000)
    {
        return x + y; // x or y is NaN
    }
    if (hx.wrapping_sub(0x3ff0_0000) as u32 | lx) == 0 {
        return atan(y); // x = 1.0
    }
    let m = ((hy >> 31) & 1) | ((hx >> 30) & 2); // 2*sign(x)+sign(y)

    // when y = 0
    if (iy as u32 | ly) == 0 {
        match m {
            0 | 1 => return y,      // atan(+-0,+anything)=+-0
            2 => return PI + TINY,  // atan(+0,-anything) = pi
            _ => return -PI - TINY, // atan(-0,-anything) =-pi
        }
    }
    // when x = 0
    if (ix as u32 | lx) == 0 {
        return if hy < 0 {
            -PI_O_2 - TINY
        } else {
            PI_O_2 + TINY
        };
    }
    // when x is INF
    if ix == 0x7ff0_0000 {
        if iy == 0x7ff0_0000 {
            return match m {
                0 => PI_O_4 + TINY,        // atan(+INF,+INF)
                1 => -PI_O_4 - TINY,       // atan(-INF,+INF)
                2 => 3.0 * PI_O_4 + TINY,  // atan(+INF,-INF)
                _ => -3.0 * PI_O_4 - TINY, // atan(-INF,-INF)
            };
        } else {
            return match m {
                0 => ZERO,       // atan(+...,+INF)
                1 => -ZERO,      // atan(-...,+INF)
                2 => PI + TINY,  // atan(+...,-INF)
                _ => -PI - TINY, // atan(-...,-INF)
            };
        }
    }
    // when y is INF
    if iy == 0x7ff0_0000 {
        return if hy < 0 {
            -PI_O_2 - TINY
        } else {
            PI_O_2 + TINY
        };
    }

    // compute y/x
    let k = (iy - ix) >> 20;
    let z = if k > 60 {
        PI_O_2 + 0.5 * PI_LO // |y/x| > 2**60
    } else if hx < 0 && k < -60 {
        0.0 // |y|/x < -2**60
    } else {
        atan((y / x).abs()) // safe to do y/x
    };
    match m {
        0 => z,                // atan(+,+)
        1 => -z,               // atan(-,+): flips the sign bit
        2 => PI - (z - PI_LO), // atan(+,-)
        _ => (z - PI_LO) - PI, // atan(-,-)
    }
}
