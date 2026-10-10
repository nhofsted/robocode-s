// SPDX-License-Identifier: EPL-2.0

//! Compares the port with `java.lang.StrictMath` from a real JDK, using vectors
//! from `tools/difftest/.../StrictMathVectors.java` (see there for the format).

use robocode_s_fdlibm as fdlibm;
use robocode_s_java_random::JavaRandom;
use std::f64::consts::PI;

const VECTORS: &str = include_str!("data/strictmath.txt");

fn unary(name: &str) -> fn(f64) -> f64 {
    match name {
        "sin" => fdlibm::sin,
        "cos" => fdlibm::cos,
        "tan" => fdlibm::tan,
        "atan" => fdlibm::atan,
        "log" => fdlibm::log,
        "toRadians" => fdlibm::to_radians,
        "toDegrees" => fdlibm::to_degrees,
        _ => panic!("unknown function {name}"),
    }
}

fn binary(name: &str) -> Option<fn(f64, f64) -> f64> {
    match name {
        "atan2" => Some(fdlibm::atan2),
        "hypot" => Some(fdlibm::hypot),
        _ => None,
    }
}

/// `Double.doubleToLongBits`: raw bits with NaN canonicalized.
fn canonical(x: f64) -> u64 {
    if x.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        x.to_bits()
    }
}

fn parse(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).unwrap())
}

fn pow2(k: i32) -> f64 {
    f64::from_bits(((1023 - k) as u64) << 52)
}

/// Input generators; must stay identical to `StrictMathVectors.generate`.
fn generate(generator: &str, r: &mut JavaRandom) -> f64 {
    match generator {
        "unit" => r.next_double() * 2.0 - 1.0,
        "ten" => (r.next_double() * 2.0 - 1.0) * 10.0,
        "million" => (r.next_double() * 2.0 - 1.0) * 1e6,
        "huge" => (r.next_double() * 2.0 - 1.0) * 1e300,
        "bits" => f64::from_bits(r.next_long() as u64),
        "positive" => f64::from_bits((r.next_long() & i64::MAX) as u64),
        "near1" => {
            let d = r.next_double() * 2.0 - 1.0;
            1.0 + d * pow2(r.next_int_bounded(40))
        }
        "small" => {
            let d = r.next_double() * 2.0 - 1.0;
            d * pow2(r.next_int_bounded(60))
        }
        "npio2" => {
            let x = f64::from(r.next_int_bounded(2_000_000) - 1_000_000) * (PI / 2.0);
            let nudge = i64::from(r.next_int_bounded(41) - 20);
            f64::from_bits((x.to_bits() as i64).wrapping_add(nudge) as u64)
        }
        _ => panic!("unknown generator {generator}"),
    }
}

fn fnv(mut hash: u64, value: f64) -> u64 {
    for byte in canonical(value).to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[test]
fn matches_strictmath() {
    let mut failures = Vec::new();
    let mut explicit = 0;
    let mut bulk = 0;
    for line in VECTORS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "v" => {
                explicit += 1;
                let (actual, expected) = match binary(f[1]) {
                    Some(op) => (op(parse(f[2]), parse(f[3])), f[4]),
                    None => (unary(f[1])(parse(f[2])), f[3]),
                };
                let expected = u64::from_str_radix(expected, 16).unwrap();
                if canonical(actual) != expected {
                    failures.push(format!(
                        "{line}: got {:x} ({actual:e}), expected {:e}",
                        canonical(actual),
                        f64::from_bits(expected)
                    ));
                }
            }
            "bulk" => {
                bulk += 1;
                let seed: i64 = f[3].parse().unwrap();
                let count: usize = f[4].parse().unwrap();
                let expected = u64::from_str_radix(f[5], 16).unwrap();
                let mut r = JavaRandom::new(seed);
                let mut hash = 0xcbf2_9ce4_8422_2325;
                match binary(f[1]) {
                    Some(op) => {
                        let (gy, gx) = f[2].split_once(':').unwrap();
                        for _ in 0..count {
                            let y = generate(gy, &mut r);
                            let x = generate(gx, &mut r);
                            hash = fnv(hash, op(y, x));
                        }
                    }
                    None => {
                        let op = unary(f[1]);
                        for _ in 0..count {
                            hash = fnv(hash, op(generate(f[2], &mut r)));
                        }
                    }
                }
                if hash != expected {
                    failures.push(format!("{line}: hash {hash:x}"));
                }
            }
            other => panic!("unknown line type {other}"),
        }
    }
    assert!(
        explicit > 5000 && bulk > 50,
        "{explicit} explicit, {bulk} bulk"
    );
    assert!(
        failures.is_empty(),
        "{} mismatches with StrictMath:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
