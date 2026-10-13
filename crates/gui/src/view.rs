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

/// Maps battlefield coordinates into a screen rectangle, keeping the aspect ratio.
#[derive(Clone, Copy)]
pub struct Transform {
    origin: Pos2,
    scale: f32,
    height: f64,
}

impl Transform {
    pub fn fit(available: Rect, width: f64, height: f64) -> (Self, Rect) {
        let scale = (available.width() / width as f32).min(available.height() / height as f32);
        let size = vec2(width as f32 * scale, height as f32 * scale);
        let rect = Rect::from_center_size(available.center(), size);
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
fn sprite(texture: egui::TextureId, center: Pos2, size: Vec2, angle: f32, tint: Color32) -> Shape {
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
        Color32::from_gray(220)
    } else {
        let [r, g, b] = sprites::rgb(c.bullet);
        Color32::from_rgb(r, g, b)
    };
    (tank, bullet)
}

/// Draws one turn into `available`; returns the battlefield's screen rectangle.
pub fn draw(
    ui: &mut egui::Ui,
    sprites: &mut Sprites,
    field: &Field,
    snapshot: Option<&v1::TurnSnapshot>,
    available: Rect,
) -> Rect {
    let (t, rect) = Transform::fit(available, field.width, field.height);
    let painter = ui.painter_at(available);
    painter.rect_filled(available, 0.0, Color32::BLACK);
    ground(&painter, sprites, t, field);
    let Some(snapshot) = snapshot else {
        return rect;
    };
    let ctx = ui.ctx().clone();

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
        bullet(&painter, sprites, t, field, snapshot, b);
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
    snapshot: &v1::TurnSnapshot,
    b: &v1::BulletSnapshot,
) {
    let at = t.point(b.x.unwrap_or_default(), b.y.unwrap_or_default());
    let state = v1::BulletState::try_from(b.state).unwrap_or(v1::BulletState::Inactive);
    match state {
        v1::BulletState::Fired | v1::BulletState::Moving => {
            let owner = usize::try_from(b.owner).ok();
            let color = owner
                .and_then(|o| Some(colors(field.robots.get(o)?, snapshot.robots.get(o)?).1))
                .unwrap_or(Color32::from_gray(220));
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
