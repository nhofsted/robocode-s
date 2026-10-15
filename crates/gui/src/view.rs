// SPDX-License-Identifier: EPL-2.0

//! The battlefield: classic's floor, tanks, bullets and explosions, robot names and
//! energy, and debug paint. Robot coordinates have y up; the screen has y down.

use crate::sprites::{self, Part, Rgb, Sprites};
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use robocode_s_protocol::v1;

/// What the view needs to know about the battle.
pub struct Field<'a> {
    pub width: f64,
    pub height: f64,
    pub robots: &'a [v1::RobotStatics],
}

/// How the battle is drawn (the options).
#[derive(Clone, Copy, Debug, Default)]
pub struct Style {
    /// Each robot's radar scan arc, as classic's "Visible scan arcs".
    pub scan_arcs: bool,
    /// Bullets in the robots' bullet colours; otherwise all in [`BULLET`].
    pub colored_bullets: bool,
}

/// Classic's bullet colour when a robot sets none.
const BULLET: Color32 = Color32::from_gray(220);

/// How far the radar sees (`Rules.RADAR_SCAN_RADIUS`).
const RADAR_RANGE: f64 = 1200.0;

/// Maps battlefield coordinates into a screen rectangle, keeping the aspect ratio.
#[derive(Clone, Copy)]
pub struct Transform {
    origin: Pos2,
    scale: f32,
    height: f64,
}

impl Transform {
    /// Fits the battlefield into `available`. An empty, inverted or non-finite area (a
    /// collapsed panel while the window is resized) or battlefield gives scale 0.
    pub fn fit(available: Rect, width: f64, height: f64) -> (Self, Rect) {
        let (w, h) = (available.width(), available.height());
        // These comparisons are false for NaN, so a NaN area gets scale 0 as well.
        let scale = if w > 0.0 && h > 0.0 && width > 0.0 && height > 0.0 {
            (w / width as f32).min(h / height as f32)
        } else {
            0.0
        };
        let scale = if scale.is_finite() { scale } else { 0.0 };
        let center = if available.center().is_finite() {
            available.center()
        } else {
            Pos2::ZERO
        };
        let size = vec2(width as f32 * scale, height as f32 * scale);
        let rect = Rect::from_center_size(center, size);
        (
            Self {
                origin: rect.min,
                scale,
                height,
            },
            rect,
        )
    }

    pub fn point(&self, x: f64, y: f64) -> Pos2 {
        pos2(
            self.origin.x + x as f32 * self.scale,
            self.origin.y + (self.height - y) as f32 * self.scale,
        )
    }

    pub fn len(&self, d: f64) -> f32 {
        d as f32 * self.scale
    }
}

/// A textured quad of `size` (screen pixels) centred on `center`, turned by `angle`
/// radians clockwise (Robocode headings: 0 is up).
pub fn sprite(
    texture: egui::TextureId,
    center: Pos2,
    size: Vec2,
    angle: f32,
    tint: Color32,
) -> Shape {
    let (sin, cos) = angle.sin_cos();
    let corner = |dx: f32, dy: f32| center + vec2(dx * cos - dy * sin, dx * sin + dy * cos);
    let (w, h) = (size.x / 2.0, size.y / 2.0);
    let mut mesh = Mesh::with_texture(texture);
    let uv = [
        pos2(0.0, 0.0),
        pos2(1.0, 0.0),
        pos2(1.0, 1.0),
        pos2(0.0, 1.0),
    ];
    let at = [corner(-w, -h), corner(w, -h), corner(w, h), corner(-w, h)];
    for (pos, uv) in at.into_iter().zip(uv) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv,
            color: tint,
        });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    Shape::mesh(mesh)
}

/// A robot's body, gun, radar and bullet colours.
pub fn colors(statics: &v1::RobotStatics, snapshot: &v1::RobotSnapshot) -> ([Rgb; 3], Color32) {
    let defaults = sprites::default_colors(&statics.full_class_name);
    let c = snapshot.colors.unwrap_or_default();
    let pick = |argb: i32, default: Rgb| {
        if argb == 0 {
            default
        } else {
            sprites::rgb(argb)
        }
    };
    let tank = [
        pick(c.body, defaults[0]),
        pick(c.gun, defaults[1]),
        pick(c.radar, defaults[2]),
    ];
    let bullet = if c.bullet == 0 {
        BULLET
    } else {
        let [r, g, b] = sprites::rgb(c.bullet);
        Color32::from_rgb(r, g, b)
    };
    (tank, bullet)
}

/// A robot's scan colour: classic's blue when it sets none.
fn scan_color(snapshot: &v1::RobotSnapshot) -> Color32 {
    match snapshot.colors.map_or(0, |c| c.scan) {
        0 => Color32::from_rgb(0, 0, 255),
        argb => {
            let [r, g, b] = sprites::rgb(argb);
            Color32::from_rgb(r, g, b)
        }
    }
}

/// A robot's scan arc this turn as (start, extent) in Robocode radians (0 up, clockwise):
/// from its radar heading in the turn before to its radar heading now, the shorter way, as
/// the engine scans (`RobotPeer.scan`). `None` when `previous` isn't the turn before.
pub fn scan_arc(
    previous: &v1::TurnSnapshot,
    current: &v1::TurnSnapshot,
    robot: usize,
) -> Option<(f64, f64)> {
    if previous.round != current.round || previous.turn + 1 != current.turn {
        return None;
    }
    let (before, now) = (previous.robots.get(robot)?, current.robots.get(robot)?);
    let alive = |r: &v1::RobotSnapshot| r.state != v1::RobotState::Dead as i32;
    if !alive(before) || !alive(now) {
        return None;
    }
    let start = before.radar_heading?;
    let mut extent = now.radar_heading? - start;
    if extent < -std::f64::consts::PI {
        extent += std::f64::consts::TAU;
    } else if extent > std::f64::consts::PI {
        extent -= std::f64::consts::TAU;
    }
    Some((start, extent))
}

/// Draws the scan arcs: classic fills them in the scan colour at 20% opacity, and draws a
/// line when the radar barely turned.
fn scan_arcs(
    painter: &egui::Painter,
    t: Transform,
    previous: &v1::TurnSnapshot,
    current: &v1::TurnSnapshot,
) {
    for (i, robot) in current.robots.iter().enumerate() {
        let Some((start, extent)) = scan_arc(previous, current, i) else {
            continue;
        };
        let (x, y) = (robot.x.unwrap_or_default(), robot.y.unwrap_or_default());
        let at = |a: f64| t.point(x + RADAR_RANGE * a.sin(), y + RADAR_RANGE * a.cos());
        let color = scan_color(robot).gamma_multiply(0.2);
        let center = t.point(x, y);
        if extent.abs() < 0.5f64.to_radians() {
            painter.line_segment([center, at(start)], Stroke::new(1.0, color));
            continue;
        }
        // At most half a turn, so the fan is convex.
        let steps = ((extent.abs() / 0.05).ceil() as usize).clamp(2, 64);
        let mut points = vec![center];
        points.extend((0..=steps).map(|k| at(start + extent * k as f64 / steps as f64)));
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
}

/// Draws one turn into `available`; returns the battlefield's screen rectangle.
/// `previous` is the turn before, for the scan arcs.
pub fn draw(
    ui: &mut egui::Ui,
    sprites: &mut Sprites,
    field: &Field,
    style: Style,
    (previous, snapshot): (Option<&v1::TurnSnapshot>, Option<&v1::TurnSnapshot>),
    available: Rect,
) -> Rect {
    let (t, rect) = Transform::fit(available, field.width, field.height);
    let painter = ui.painter_at(available);
    painter.rect_filled(available, 0.0, Color32::BLACK);
    if t.scale == 0.0 {
        return rect;
    }
    ground(&painter, sprites, t, field);
    let Some(snapshot) = snapshot else {
        return rect;
    };
    let ctx = ui.ctx().clone();

    if style.scan_arcs
        && let Some(previous) = previous
    {
        scan_arcs(&painter.with_clip_rect(rect), t, previous, snapshot);
    }
    // Debug paint first, under the tanks, as in classic.
    for (r, robot) in snapshot.robots.iter().enumerate() {
        if !robot.paint.is_empty() {
            paint(&painter, t, &robot.paint, r);
        }
    }
    for (i, robot) in snapshot.robots.iter().enumerate() {
        if robot.state == v1::RobotState::Dead as i32 {
            continue;
        }
        let statics = &field.robots[i];
        let (tank, _) = colors(statics, robot);
        let center = t.point(robot.x.unwrap_or_default(), robot.y.unwrap_or_default());
        for (part, color, heading) in [
            (Part::Body, tank[0], robot.body_heading),
            (Part::Gun, tank[1], robot.gun_heading),
            (Part::Radar, tank[2], robot.radar_heading),
        ] {
            let (texture, size) = sprites.tank(&ctx, part, color);
            painter.add(sprite(
                texture,
                center,
                size * t.scale,
                heading.unwrap_or_default() as f32,
                Color32::WHITE,
            ));
        }
        let font = egui::FontId::proportional((11.0 * t.scale).clamp(8.0, 14.0));
        let half = t.len(24.0);
        painter.text(
            center - vec2(0.0, half),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.1}", robot.energy.unwrap_or_default()),
            font.clone(),
            Color32::WHITE,
        );
        painter.text(
            center + vec2(0.0, half),
            egui::Align2::CENTER_TOP,
            &statics.name,
            font,
            Color32::WHITE,
        );
    }
    for b in &snapshot.bullets {
        bullet(&painter, sprites, t, field, style, snapshot, b);
    }
    rect
}

fn ground(painter: &egui::Painter, sprites: &Sprites, t: Transform, field: &Field) {
    // Classic's BattleView: one of the five tiles per 64-unit cell, picked at random; a fixed
    // seed here, as on the website, so the floor doesn't flicker.
    const CELL: f64 = 64.0;
    let mut seed: u32 = 20010101;
    let cols = (field.width / CELL).ceil() as usize;
    let rows = (field.height / CELL).ceil() as usize;
    let clip = Rect::from_min_max(t.point(0.0, field.height), t.point(field.width, 0.0));
    let painter = painter.with_clip_rect(clip);
    for row in 0..rows {
        for col in 0..cols {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let tile = ((f64::from(seed) / 4294967296.0) * 4.0).round() as usize;
            let min = t.point(col as f64 * CELL, field.height - row as f64 * CELL);
            let rect = Rect::from_min_size(min, Vec2::splat(t.len(CELL)));
            painter.image(
                sprites.ground[tile].id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

fn bullet(
    painter: &egui::Painter,
    sprites: &Sprites,
    t: Transform,
    field: &Field,
    style: Style,
    snapshot: &v1::TurnSnapshot,
    b: &v1::BulletSnapshot,
) {
    let at = t.point(b.x.unwrap_or_default(), b.y.unwrap_or_default());
    let state = v1::BulletState::try_from(b.state).unwrap_or(v1::BulletState::Inactive);
    match state {
        v1::BulletState::Fired | v1::BulletState::Moving => {
            let owner = usize::try_from(b.owner)
                .ok()
                .filter(|_| style.colored_bullets);
            let color = owner
                .and_then(|o| Some(colors(field.robots.get(o)?, snapshot.robots.get(o)?).1))
                .unwrap_or(BULLET);
            // Classic: a disc of 2·√(2.5·power), at least 2 screen pixels.
            let d = t.len(2.0 * (2.5 * b.power.unwrap_or(1.0)).sqrt()).max(2.0);
            painter.circle_filled(at, d / 2.0, color);
        }
        v1::BulletState::HitVictim
        | v1::BulletState::HitBullet
        | v1::BulletState::HitWall
        | v1::BulletState::Exploded => {
            let frames = sprites.explosion.len();
            // Classic scales a hit by its power (√(1000·power)/128 of the 128-px frames). A
            // robot's death is classic's larger explosion, drawn here with the same frames,
            // slower and at full size.
            let (frame, size) = if b.explosion {
                ((b.frame / 4) as usize, 1.0)
            } else {
                let power = b.power.unwrap_or(1.0);
                (b.frame as usize, ((1000.0 * power).sqrt() / 128.0) as f32)
            };
            if let Some(texture) = sprites.explosion.get(frame.min(frames - 1)) {
                let s = texture.size_vec2() * t.scale * size;
                painter.image(
                    texture.id(),
                    Rect::from_center_size(at, s),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        v1::BulletState::Inactive => {}
    }
}

/// A robot's debug paint (`PaintOp`s, battlefield coordinates).
fn paint(painter: &egui::Painter, t: Transform, ops: &[v1::PaintOp], _robot: usize) {
    use v1::paint_op::Op;
    let mut color = Color32::WHITE;
    let mut width = 1.0f32;
    let mut font_size = 12.0f32;
    for op in ops {
        match &op.op {
            Some(Op::SetColor(argb)) => {
                let v = *argb as u32;
                color = Color32::from_rgba_unmultiplied(
                    (v >> 16) as u8,
                    (v >> 8) as u8,
                    v as u8,
                    (v >> 24) as u8,
                );
            }
            Some(Op::SetStrokeWidth(w)) => width = *w,
            Some(Op::SetFontSize(s)) => font_size = *s,
            Some(Op::Draw(shape)) => {
                for points in outline(t, shape) {
                    let (points, closed) = points;
                    let stroke = Stroke::new((t.scale * width).max(1.0), color);
                    if closed {
                        painter.add(Shape::closed_line(points, stroke));
                    } else {
                        painter.add(Shape::line(points, stroke));
                    }
                }
            }
            Some(Op::Fill(shape)) => {
                for (points, _) in outline(t, shape) {
                    if points.len() >= 3 {
                        // Fill as a fan; robots fill simple (convex) shapes.
                        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
                    }
                }
            }
            Some(Op::DrawString(s)) => {
                // Java's drawString puts the baseline at y (y up here).
                painter.text(
                    t.point(s.x.unwrap_or_default(), s.y.unwrap_or_default()),
                    egui::Align2::LEFT_BOTTOM,
                    &s.text,
                    egui::FontId::monospace((font_size * t.scale).max(6.0)),
                    color,
                );
            }
            None => {}
        }
    }
}

/// A shape's outline in screen points, and whether it is closed.
fn outline(t: Transform, shape: &v1::Shape) -> Vec<(Vec<Pos2>, bool)> {
    use v1::shape::Shape as S;
    let ellipse = |r: &v1::Rect, start: f64, extent: f64, closed: bool| {
        let (x, y) = (r.x.unwrap_or_default(), r.y.unwrap_or_default());
        let (w, h) = (r.width.unwrap_or_default(), r.height.unwrap_or_default());
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let steps = 48;
        let points = (0..=steps)
            .map(|i| {
                // Arc2D: degrees counter-clockwise from 3 o'clock, in a y-down frame;
                // robot paint has y up, so the arc mirrors.
                let a = (start + extent * f64::from(i) / f64::from(steps)).to_radians();
                t.point(cx + w / 2.0 * a.cos(), cy - h / 2.0 * a.sin())
            })
            .collect();
        (points, closed)
    };
    match &shape.shape {
        Some(S::Line(l)) => vec![(
            vec![
                t.point(l.x1.unwrap_or_default(), l.y1.unwrap_or_default()),
                t.point(l.x2.unwrap_or_default(), l.y2.unwrap_or_default()),
            ],
            false,
        )],
        Some(S::Rect(r)) => {
            let (x, y) = (r.x.unwrap_or_default(), r.y.unwrap_or_default());
            let (w, h) = (r.width.unwrap_or_default(), r.height.unwrap_or_default());
            vec![(
                vec![
                    t.point(x, y),
                    t.point(x + w, y),
                    t.point(x + w, y + h),
                    t.point(x, y + h),
                ],
                true,
            )]
        }
        Some(S::Oval(r)) => vec![ellipse(r, 0.0, 360.0, true)],
        Some(S::Arc(a)) => match &a.frame {
            Some(frame) => vec![ellipse(
                frame,
                a.start.unwrap_or_default(),
                a.extent.unwrap_or_default(),
                false,
            )],
            None => Vec::new(),
        },
        Some(S::Polygon(p)) => vec![(
            p.xs.iter()
                .zip(&p.ys)
                .map(|(&x, &y)| t.point(x, y))
                .collect(),
            !p.open,
        )],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_aspect_ratio() {
        let available = Rect::from_min_size(pos2(10.0, 20.0), vec2(1000.0, 600.0));
        let (t, rect) = Transform::fit(available, 800.0, 600.0);
        assert_eq!(t.scale, 1.0);
        assert_eq!(
            rect,
            Rect::from_min_size(pos2(110.0, 20.0), vec2(800.0, 600.0))
        );
        assert_eq!(t.point(0.0, 0.0), pos2(110.0, 620.0));
    }

    #[test]
    fn scan_arcs_go_from_the_last_radar_heading_the_shorter_way() {
        let turn = |turn: i64, headings: &[f64]| v1::TurnSnapshot {
            round: 0,
            turn,
            robots: headings
                .iter()
                .map(|&h| v1::RobotSnapshot {
                    radar_heading: Some(h),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let (a, b) = (turn(4, &[0.1, 6.2]), turn(5, &[0.5, 0.1]));
        let (start, extent) = scan_arc(&a, &b, 0).unwrap();
        assert_eq!(start, 0.1);
        assert!((extent - 0.4).abs() < 1e-12);
        // Across north: clockwise through 0, not back around.
        let (start, extent) = scan_arc(&a, &b, 1).unwrap();
        assert_eq!(start, 6.2);
        assert!((extent - (0.1 + std::f64::consts::TAU - 6.2)).abs() < 1e-12);
        // Not the turn before: no arc.
        assert_eq!(scan_arc(&turn(3, &[0.1]), &b, 0), None);
        assert_eq!(scan_arc(&a, &b, 2), None);
    }

    #[test]
    fn fit_survives_degenerate_areas() {
        let degenerate = [
            Rect::from_min_size(pos2(5.0, 5.0), Vec2::ZERO),
            Rect::from_min_size(pos2(5.0, 5.0), vec2(300.0, 0.0)),
            Rect::from_min_max(pos2(100.0, 100.0), pos2(40.0, 30.0)),
            Rect::NOTHING,
            Rect::EVERYTHING,
            Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(10.0, 10.0)),
        ];
        for available in degenerate {
            for (w, h) in [(800.0, 600.0), (0.0, 0.0), (-1.0, 600.0)] {
                let (t, rect) = Transform::fit(available, w, h);
                assert!(
                    t.scale.is_finite() && t.scale >= 0.0,
                    "{available:?} {w}x{h}"
                );
                assert!(
                    rect.width() >= 0.0 && rect.height() >= 0.0,
                    "{available:?} {w}x{h} {rect:?} {}",
                    t.scale
                );
                assert!(t.point(400.0, 300.0).is_finite(), "{available:?} {w}x{h}");
            }
        }
    }
}
