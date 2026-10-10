// SPDX-License-Identifier: EPL-2.0

//! Compares the geometry with `java.awt.geom` from a real JDK, using vectors from
//! `tools/difftest/.../GeometryVectors.java` (see there for the format).

use robocode_s_fdlibm as fdlibm;
use robocode_s_geom::{Arc, Rect, scan_intersects};
use robocode_s_java_random::JavaRandom;
use std::collections::BTreeMap;
use std::f64::consts::PI;

const VECTORS: &str = include_str!("data/geometry.txt");

/// A double written by Java's `Double.toString` (shortest round-trip form).
fn d(text: &str) -> f64 {
    match text {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ => text.parse().unwrap(),
    }
}

/// `Utils.normalAbsoluteAngle`.
fn normal_absolute_angle(angle: f64) -> f64 {
    let a = angle % (2.0 * PI);
    if a >= 0.0 { a } else { a + 2.0 * PI }
}

/// The arc of classic's `RobotPeer.scan` (engine-spec §7).
fn scan_arc(x: f64, y: f64, last_radar: f64, radar: f64) -> Arc {
    let mut start = last_radar;
    let mut scan = radar - start;
    if scan < -PI {
        scan += 2.0 * PI;
    } else if scan > PI {
        scan -= 2.0 * PI;
    }
    start -= PI / 2.0;
    start = normal_absolute_angle(start);
    Arc {
        x: x - 1200.0,
        y: y - 1200.0,
        w: 2400.0,
        h: 2400.0,
        start: 180.0 * start / PI,
        extent: 180.0 * scan / PI,
    }
}

fn scan(x: f64, y: f64, last_radar: f64, radar: f64, ox: f64, oy: f64) -> bool {
    scan_intersects(&scan_arc(x, y, last_radar, radar), &Rect::robot(ox, oy))
}

/// Random cases; must stay identical to `GeometryVectors.bulkCase`.
fn bulk_case(kind: &str, r: &mut JavaRandom) -> bool {
    match kind {
        "box" => {
            let (ax, ay) = (r.next_double() * 800.0, r.next_double() * 600.0);
            let bx = ax + (r.next_double() * 2.0 - 1.0) * 40.0;
            let by = ay + (r.next_double() * 2.0 - 1.0) * 40.0;
            Rect::robot(ax, ay).intersects(&Rect::robot(bx, by))
        }
        "line" => {
            let (cx, cy) = (r.next_double() * 800.0, r.next_double() * 600.0);
            let x1 = cx + (r.next_double() * 2.0 - 1.0) * 40.0;
            let y1 = cy + (r.next_double() * 2.0 - 1.0) * 40.0;
            let a = r.next_double() * 2.0 * PI;
            let v = 11.0 + r.next_double() * 8.7;
            Rect::robot(cx, cy).intersects_line(
                x1,
                y1,
                x1 + v * fdlibm::sin(a),
                y1 + v * fdlibm::cos(a),
            )
        }
        "scan" => {
            let (x, y) = (r.next_double() * 800.0, r.next_double() * 600.0);
            let h = r.next_double() * 2.0 * PI;
            let delta = (r.next_double() * 2.0 - 1.0) * PI / 4.0;
            let (ox, oy) = (r.next_double() * 800.0, r.next_double() * 600.0);
            scan(x, y, h, h + delta, ox, oy)
        }
        "arc" => {
            let start = r.next_double() * 720.0 - 360.0;
            let extent = r.next_double() * 760.0 - 380.0;
            let (rx, ry) = (
                r.next_double() * 300.0 - 150.0,
                r.next_double() * 300.0 - 150.0,
            );
            let (rw, rh) = (r.next_double() * 60.0, r.next_double() * 60.0);
            let arc = Arc {
                x: -100.0,
                y: -100.0,
                w: 200.0,
                h: 200.0,
                start,
                extent,
            };
            arc.intersects(rx, ry, rw, rh)
        }
        "ellipse" => {
            let (x, y) = (
                r.next_double() * 200.0 - 100.0,
                r.next_double() * 200.0 - 100.0,
            );
            let (w, h) = (r.next_double() * 300.0, r.next_double() * 300.0);
            let start = r.next_double() * 720.0 - 360.0;
            let extent = r.next_double() * 760.0 - 380.0;
            let (rx, ry) = (
                r.next_double() * 400.0 - 200.0,
                r.next_double() * 400.0 - 200.0,
            );
            let (rw, rh) = (r.next_double() * 100.0, r.next_double() * 100.0);
            Arc {
                x,
                y,
                w,
                h,
                start,
                extent,
            }
            .intersects(rx, ry, rw, rh)
        }
        _ => panic!("unknown bulk kind {kind}"),
    }
}

#[test]
fn matches_java_awt_geom() {
    let mut checked: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in VECTORS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split(' ').collect();
        let kind = f[0];
        let ok = match kind {
            "box" => {
                Rect::robot(d(f[1]), d(f[2])).intersects(&Rect::robot(d(f[3]), d(f[4])))
                    == (f[5] == "true")
            }
            "contains" => {
                Rect::robot(d(f[1]), d(f[2])).contains(d(f[3]), d(f[4])) == (f[5] == "true")
            }
            "line" => {
                Rect::robot(d(f[1]), d(f[2])).intersects_line(d(f[3]), d(f[4]), d(f[5]), d(f[6]))
                    == (f[7] == "true")
            }
            "arc" => {
                let arc = Arc {
                    x: d(f[1]),
                    y: d(f[2]),
                    w: d(f[3]),
                    h: d(f[4]),
                    start: d(f[5]),
                    extent: d(f[6]),
                };
                arc.intersects(d(f[7]), d(f[8]), d(f[9]), d(f[10])) == (f[11] == "true")
            }
            "start" => {
                let arc = Arc {
                    x: d(f[1]),
                    y: d(f[2]),
                    w: d(f[3]),
                    h: d(f[4]),
                    start: d(f[5]),
                    extent: d(f[6]),
                };
                let (px, py) = arc.start_point();
                px.to_bits() == d(f[7]).to_bits() && py.to_bits() == d(f[8]).to_bits()
            }
            "scan" => {
                scan(d(f[1]), d(f[2]), d(f[3]), d(f[4]), d(f[5]), d(f[6])) == (f[7] == "true")
            }
            "bulk" => {
                let mut r = JavaRandom::new(f[2].parse().unwrap());
                let count: usize = f[3].parse().unwrap();
                let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
                for _ in 0..count {
                    hash ^= u64::from(bulk_case(f[1], &mut r));
                    hash = hash.wrapping_mul(0x100_0000_01b3);
                }
                hash == u64::from_str_radix(f[4], 16).unwrap()
            }
            other => panic!("unknown line type {other}"),
        };
        let key = if kind == "bulk" {
            format!("bulk {}", f[1])
        } else {
            kind.to_string()
        };
        *checked.entry(key.clone()).or_default() += 1;
        if !ok {
            failures.entry(key).or_default().push(line.to_string());
        }
    }
    if let Ok(path) = std::env::var("GEOM_DUMP") {
        let all: Vec<String> = failures.values().flatten().cloned().collect();
        std::fs::write(path, all.join("\n")).unwrap();
    }
    let summary: Vec<String> = checked
        .iter()
        .map(|(k, n)| format!("{k}: {} of {n} differ", failures.get(k).map_or(0, Vec::len)))
        .collect();
    let examples: Vec<String> = failures
        .values()
        .flat_map(|lines| lines.iter().take(5).cloned())
        .collect();
    assert!(
        failures.is_empty(),
        "mismatches with java.awt.geom:\n{}\nexamples:\n{}",
        summary.join("\n"),
        examples.join("\n")
    );
}
