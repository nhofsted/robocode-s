// SPDX-License-Identifier: EPL-2.0

//! Classic's `robocode.util.Utils` angle helpers (engine-spec §1.3).

use std::f64::consts::PI;

const TWO_PI: f64 = 2.0 * PI;
const PI_OVER_TWO: f64 = PI / 2.0;
const THREE_PI_OVER_TWO: f64 = 3.0 * PI / 2.0;
pub const NEAR_DELTA: f64 = 0.00001;

pub fn normal_absolute_angle(angle: f64) -> f64 {
    let a = angle % TWO_PI;
    if a >= 0.0 { a } else { a + TWO_PI }
}

pub fn normal_relative_angle(angle: f64) -> f64 {
    let a = angle % TWO_PI;
    if a >= 0.0 {
        if a < PI { a } else { a - TWO_PI }
    } else if a >= -PI {
        a
    } else {
        a + TWO_PI
    }
}

pub fn is_near(a: f64, b: f64) -> bool {
    (a - b).abs() < NEAR_DELTA
}

pub fn normal_near_absolute_angle(angle: f64) -> f64 {
    let a = normal_absolute_angle(angle);
    if is_near(a, PI) {
        return PI;
    } else if a < PI {
        if is_near(a, 0.0) {
            return 0.0;
        } else if is_near(a, PI_OVER_TWO) {
            return PI_OVER_TWO;
        }
    } else if is_near(a, THREE_PI_OVER_TWO) {
        return THREE_PI_OVER_TWO;
    } else if is_near(a, TWO_PI) {
        return 0.0;
    }
    a
}
