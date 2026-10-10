// SPDX-License-Identifier: EPL-2.0

//! The geometry tests classic Robocode uses for scanning and collisions,
//! reproducing the observable behavior of `java.awt.geom` (engine-spec §7, §9.1).
//!
//! This is an independent implementation (ADR 0011): OpenJDK's source is GPL and
//! was not used. The behavior was established by black-box experiments and is
//! verified against a real JDK with explicit edge cases and millions of random
//! cases (`tests/jdk_geometry.rs`, vectors from `tools/difftest`).

use robocode_s_fdlibm as fdlibm;

/// A robot bounding box: classic's `BoundingRectangle`, a `Rectangle2D.Float`.
/// Coordinates are stored in single precision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const OUT_LEFT: u8 = 1;
const OUT_TOP: u8 = 2;
const OUT_RIGHT: u8 = 4;
const OUT_BOTTOM: u8 = 8;

impl Rect {
    /// The empty box robots start with.
    pub const EMPTY: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    /// `setRect(x, y, w, h)`: every value is rounded to `f32`.
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
        }
    }

    /// The 36×36 box of a robot centered at `(x, y)`.
    pub fn robot(x: f64, y: f64) -> Self {
        Self::new(x - 18.0, y - 18.0, 36.0, 36.0)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    fn x0(&self) -> f64 {
        f64::from(self.x)
    }

    fn y0(&self) -> f64 {
        f64::from(self.y)
    }

    fn x1(&self) -> f64 {
        f64::from(self.x) + f64::from(self.w)
    }

    fn y1(&self) -> f64 {
        f64::from(self.y) + f64::from(self.h)
    }

    /// `intersects(Rectangle2D)`: the open boxes overlap. Empty boxes never intersect.
    pub fn intersects(&self, other: &Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && other.x1() > self.x0()
            && other.y1() > self.y0()
            && other.x0() < self.x1()
            && other.y0() < self.y1()
    }

    /// `contains(px, py)`: the half-open box `[x, x+w) × [y, y+h)`.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x0() && py >= self.y0() && px < self.x1() && py < self.y1()
    }

    /// `intersectsLine(x1, y1, x2, y2)`: whether the segment meets the closed box.
    pub fn intersects_line(&self, x1: f64, y1: f64, x2: f64, y2: f64) -> bool {
        Bounds {
            x0: self.x0(),
            y0: self.y0(),
            x1: self.x1(),
            y1: self.y1(),
            no_width: self.w <= 0.0,
            no_height: self.h <= 0.0,
        }
        .intersects_line(x1, y1, x2, y2)
    }
}

/// A box in double precision, for line tests.
struct Bounds {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    no_width: bool,
    no_height: bool,
}

impl Bounds {
    /// Points on the edges count as inside; a box without width (height) has every point
    /// both left and right (above and below). This is robocode's `BoundingRectangle.outcode`.
    fn outcode(&self, px: f64, py: f64) -> u8 {
        let mut out = 0;
        if self.no_width {
            out |= OUT_LEFT | OUT_RIGHT;
        } else if px < self.x0 {
            out |= OUT_LEFT;
        } else if px > self.x1 {
            out |= OUT_RIGHT;
        }
        if self.no_height {
            out |= OUT_TOP | OUT_BOTTOM;
        } else if py < self.y0 {
            out |= OUT_TOP;
        } else if py > self.y1 {
            out |= OUT_BOTTOM;
        }
        out
    }

    /// Cohen–Sutherland clipping: the segment's first endpoint is moved onto box edges
    /// (vertical edges first) until it is inside, or both endpoints are outside on the
    /// same side.
    fn intersects_line(&self, mut x1: f64, mut y1: f64, x2: f64, y2: f64) -> bool {
        let out2 = self.outcode(x2, y2);
        if out2 == 0 {
            return true;
        }
        loop {
            let out1 = self.outcode(x1, y1);
            if out1 == 0 {
                return true;
            }
            if out1 & out2 != 0 {
                return false;
            }
            if out1 & (OUT_LEFT | OUT_RIGHT) != 0 {
                let edge = if out1 & OUT_RIGHT != 0 {
                    self.x1
                } else {
                    self.x0
                };
                y1 += (edge - x1) * (y2 - y1) / (x2 - x1);
                x1 = edge;
            } else {
                let edge = if out1 & OUT_BOTTOM != 0 {
                    self.y1
                } else {
                    self.y0
                };
                x1 += (edge - y1) * (x2 - x1) / (y2 - y1);
                y1 = edge;
            }
        }
    }
}

/// A pie-shaped `Arc2D.Double` (type `PIE`): an ellipse in the frame `(x, y, w, h)`
/// cut to the angles `start .. start + extent` (degrees). Angles follow `Arc2D`:
/// 0° points to +x and positive angles turn towards -y.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub start: f64,
    pub extent: f64,
}

impl Arc {
    pub fn center_x(&self) -> f64 {
        self.x + self.w / 2.0
    }

    pub fn center_y(&self) -> f64 {
        self.y + self.h / 2.0
    }

    /// The point on the ellipse at angle `degrees`.
    fn point_at(&self, degrees: f64) -> (f64, f64) {
        let angle = fdlibm::to_radians(-degrees);
        (
            self.x + (fdlibm::cos(angle) * 0.5 + 0.5) * self.w,
            self.y + (fdlibm::sin(angle) * 0.5 + 0.5) * self.h,
        )
    }

    /// `getStartPoint()`.
    pub fn start_point(&self) -> (f64, f64) {
        self.point_at(self.start)
    }

    fn is_full(&self) -> bool {
        self.extent.abs() >= 360.0
    }

    /// `containsAngle(angle)`: the start ray belongs to the arc and the end ray doesn't,
    /// in both directions. The angle and the start are each normalized to (-180, 180]
    /// before they are compared; this matches the JDK's rounding exactly.
    pub fn contains_angle(&self, angle: f64) -> bool {
        let size = self.extent.abs();
        if size >= 360.0 {
            return true;
        }
        let mut d = normalize_degrees(angle) - normalize_degrees(self.start);
        if self.extent < 0.0 {
            d = -d;
        }
        if d < 0.0 {
            d += 360.0;
        }
        d >= 0.0 && d < size
    }

    /// The point relative to the frame: `(0, 0)` is the center, `(±0.5, ±0.5)` the corners.
    fn offsets(&self, px: f64, py: f64) -> (f64, f64) {
        ((px - self.x) / self.w - 0.5, (py - self.y) / self.h - 0.5)
    }

    /// The `Arc2D` angle (degrees) of a point given by its frame offsets.
    fn angle_of(nx: f64, ny: f64) -> f64 {
        -fdlibm::to_degrees(fdlibm::atan2(ny, nx))
    }

    /// `contains(px, py)`: inside the open ellipse and within the angles.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        if self.w <= 0.0 || self.h <= 0.0 {
            return false;
        }
        let (nx, ny) = self.offsets(px, py);
        if nx * nx + ny * ny >= 0.25 {
            return false;
        }
        self.is_full() || self.contains_angle(Self::angle_of(nx, ny))
    }

    /// Whether the direction from the center to `(px, py)` is within the angles.
    fn direction_inside(&self, px: f64, py: f64) -> bool {
        let (nx, ny) = self.offsets(px, py);
        self.is_full() || self.contains_angle(Self::angle_of(nx, ny))
    }

    /// `intersects(rx, ry, rw, rh)` for a pie. Boundary behavior, as observed on the JDK:
    /// an extent of 0 never intersects; the radial edges count as closed segments; the
    /// curved edge and the rectangle's edges are open.
    pub fn intersects(&self, rx: f64, ry: f64, rw: f64, rh: f64) -> bool {
        if rw <= 0.0 || rh <= 0.0 || self.w <= 0.0 || self.h <= 0.0 || self.extent == 0.0 {
            return false;
        }
        let (rx1, ry1) = (rx + rw, ry + rh);
        // Outside the ellipse's frame.
        if rx1 <= self.x || ry1 <= self.y || rx >= self.x + self.w || ry >= self.y + self.h {
            return false;
        }
        let (cx, cy) = (self.center_x(), self.center_y());
        // A radial edge meets the closed rectangle. Full ellipses are tested too.
        let bounds = Bounds {
            x0: rx,
            y0: ry,
            x1: rx1,
            y1: ry1,
            no_width: false,
            no_height: false,
        };
        for angle in [self.start, self.start + self.extent] {
            let (ex, ey) = self.point_at(angle);
            if bounds.intersects_line(cx, cy, ex, ey) {
                return true;
            }
        }
        // A rectangle corner inside the pie.
        for (px, py) in [(rx, ry), (rx1, ry), (rx, ry1), (rx1, ry1)] {
            if self.contains(px, py) {
                return true;
            }
        }
        // The curved edge of the pie through a side of the rectangle.
        let (a, b) = (self.w / 2.0, self.h / 2.0);
        for side_x in [rx, rx1] {
            let u = (side_x - cx) / a;
            if u > -1.0 && u < 1.0 {
                let dv = (1.0 - u * u).sqrt() * b;
                for py in [cy - dv, cy + dv] {
                    if py > ry && py < ry1 && self.direction_inside(side_x, py) {
                        return true;
                    }
                }
            }
        }
        for side_y in [ry, ry1] {
            let v = (side_y - cy) / b;
            if v > -1.0 && v < 1.0 {
                let du = (1.0 - v * v).sqrt() * a;
                for px in [cx - du, cx + du] {
                    if px > rx && px < rx1 && self.direction_inside(px, side_y) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Degrees normalized to `(-180, 180]`: one step of 360 when that suffices, otherwise
/// the IEEE remainder.
fn normalize_degrees(angle: f64) -> f64 {
    let r = if angle > 180.0 {
        if angle <= 540.0 {
            return angle - 360.0;
        }
        ieee_remainder_360(angle)
    } else if angle <= -180.0 {
        if angle > -540.0 {
            return angle + 360.0;
        }
        ieee_remainder_360(angle)
    } else {
        return angle;
    };
    if r == -180.0 { 180.0 } else { r }
}

/// IEEE 754 remainder of `x` by 360: `x - n * 360` with `n` the integer nearest to
/// `x / 360`, ties to even. Every step is exact.
fn ieee_remainder_360(x: f64) -> f64 {
    let r = x % 360.0; // exact, |r| < 360, sign of x
    if r.abs() > 180.0 || (r.abs() == 180.0 && (x % 720.0).abs() >= 360.0) {
        r - 360.0f64.copysign(r)
    } else {
        r
    }
}

/// Classic's radar test (`RobotPeer.intersects`, engine-spec §7): the box meets the
/// line from the arc's center to its start point, or the pie itself.
pub fn scan_intersects(arc: &Arc, rect: &Rect) -> bool {
    let (sx, sy) = arc.start_point();
    rect.intersects_line(arc.center_x(), arc.center_y(), sx, sy)
        || arc.intersects(
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.w),
            f64::from(rect.h),
        )
}
