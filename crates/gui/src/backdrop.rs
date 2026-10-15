// SPDX-License-Identifier: EPL-2.0

//! The background of the pages: classic's floor, darkened, with a few tanks wandering,
//! sweeping their radars and firing, as on the website's home page (a port of
//! `web/src/lib/components/Battlefield.svelte`). Not the engine, just a calm backdrop: slow,
//! at most 30 frames a second, and still when there are no margins to show it in.

use crate::sprites::{Part, Rgb, Sprites};
use crate::view::sprite;
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Vec2, pos2, vec2};
use std::f32::consts::TAU;
use std::time::Duration;

const COLORS: [[Rgb; 3]; 5] = [
    [[63, 138, 79], [159, 212, 168], [255, 215, 94]],
    [[160, 50, 50], [217, 160, 160], [94, 227, 214]],
    [[50, 90, 160], [164, 189, 224], [255, 179, 94]],
    [[150, 125, 40], [216, 204, 152], [255, 122, 240]],
    [[100, 60, 160], [192, 168, 230], [156, 255, 94]],
];
const TANKS: usize = 6;
/// Robots are 36 × 36 boxes, as in classic.
const SIZE: f32 = 36.0;
/// Simulation speed relative to one step per 60 Hz frame: calm enough for a background.
/// A little slower than the website's 0.35.
const PACE: f32 = 0.3;
/// Darkens the floor, like the website's `--ground-shade` (rgb(8 11 18) at 0.6, a little
/// more than on its home page, as here the backdrop sits beside the controls).
const SHADE: Color32 = Color32::from_rgba_premultiplied(5, 7, 11, 153);
/// Classic's bullet explosion has 17 frames.
const EXPLOSION_FRAMES: f32 = 17.0;
const BULLET: Color32 = Color32::from_rgb(255, 194, 92);
/// Margins narrower than this (each side) aren't worth animating.
const MIN_MARGIN: f32 = 40.0;

struct Bot {
    pos: Pos2,
    heading: f32,
    gun: f32,
    radar: f32,
    speed: f32,
    turn: f32,
    heat: f32,
    colors: [Rgb; 3],
    target: usize,
}

struct Bullet {
    pos: Pos2,
    velocity: Vec2,
    owner: usize,
    power: f32,
}

struct Blast {
    pos: Pos2,
    frame: f32,
    scale: f32,
}

pub struct Backdrop {
    bots: Vec<Bot>,
    bullets: Vec<Bullet>,
    blasts: Vec<Blast>,
    seed: u32,
    /// The simulated area: the page's size when it was laid out last.
    size: Vec2,
    last: Option<f64>,
}

impl Default for Backdrop {
    fn default() -> Self {
        Self {
            bots: Vec::new(),
            bullets: Vec::new(),
            blasts: Vec::new(),
            seed: 7,
            size: Vec2::ZERO,
            last: None,
        }
    }
}

/// The angle from `a` to `b`, Robocode style: 0 is up, clockwise (screen y is down).
fn angle_to(a: Pos2, b: Pos2) -> f32 {
    (b.x - a.x).atan2(a.y - b.y)
}

/// `a` in (-π, π].
fn norm(a: f32) -> f32 {
    a.sin().atan2(a.cos())
}

/// A unit vector along a Robocode heading.
fn ahead(heading: f32) -> Vec2 {
    vec2(heading.sin(), -heading.cos())
}

impl Backdrop {
    /// Park–Miller, as on the website.
    fn rand(&mut self) -> f32 {
        self.seed = ((u64::from(self.seed) * 16807) % 2147483647) as u32;
        self.seed as f32 / 2147483647.0
    }

    fn spawn(&mut self) {
        let (w, h) = (self.size.x, self.size.y);
        let mut tries = 0;
        while self.bots.len() < TANKS {
            tries += 1;
            let x = 60.0 + self.rand() * (w - 120.0).max(1.0);
            let y = 60.0 + self.rand() * (h - 120.0).max(1.0);
            // Start apart (a few tries, then accept whatever space there is).
            let close = |b: &Bot| (b.pos.x - x).abs() < 80.0 && (b.pos.y - y).abs() < 80.0;
            if tries < 200 && self.bots.iter().any(close) {
                continue;
            }
            let i = self.bots.len();
            let (heading, gun, radar, heat) = (
                self.rand() * TAU,
                self.rand() * TAU,
                self.rand() * TAU,
                1.0 + self.rand() * 2.0,
            );
            self.bots.push(Bot {
                pos: pos2(x, y),
                heading,
                gun,
                radar,
                speed: 0.0,
                turn: 0.0,
                heat,
                colors: COLORS[(i + 1) % COLORS.len()],
                target: (i + 1) % TANKS,
            });
        }
    }

    /// Advances the simulation by `dt` steps; `real` is the frame time in 60 Hz frames.
    fn step(&mut self, dt: f32, real: f32) {
        let (w, h) = (self.size.x, self.size.y);
        for i in 0..self.bots.len() {
            if self.rand() < 0.02 {
                self.bots[i].turn = (self.rand() - 0.5) * 0.08;
            }
            if self.rand() < 0.02 {
                let sign = if self.rand() < 0.5 { -1.0 } else { 1.0 };
                self.bots[i].speed = sign * (2.0 + self.rand() * 4.0);
            }
            // Steer away from walls.
            let b = &mut self.bots[i];
            let margin = 50.0;
            if b.pos.x < margin || b.pos.x > w - margin || b.pos.y < margin || b.pos.y > h - margin
            {
                let to_center = angle_to(b.pos, pos2(w / 2.0, h / 2.0));
                b.turn = norm(to_center - b.heading).signum() * 0.08;
                b.speed = if b.speed == 0.0 { 3.0 } else { b.speed.abs() };
            }
            b.heading += b.turn * dt;
            let next = b.pos + ahead(b.heading) * b.speed * dt;
            let next = pos2(
                next.x.clamp(20.0, (w - 20.0).max(20.0)),
                next.y.clamp(20.0, (h - 20.0).max(20.0)),
            );
            // A move that pushes into another robot is blocked, like classic's ram; then back
            // off and turn away.
            let pos = b.pos;
            let blocked = self.bots.iter().enumerate().any(|(j, o)| {
                j != i
                    && (o.pos.x - next.x).abs() < SIZE
                    && (o.pos.y - next.y).abs() < SIZE
                    && o.pos.distance(next) < o.pos.distance(pos)
            });
            if blocked {
                let ahead = if self.bots[i].speed < 0.0 { -1.0 } else { 1.0 };
                let speed = -ahead * (1.5 + self.rand() * 2.0);
                let sign = if self.rand() < 0.5 { -1.0 } else { 1.0 };
                let turn = sign * (0.03 + self.rand() * 0.05);
                let b = &mut self.bots[i];
                b.speed = speed;
                b.turn = turn;
            } else {
                self.bots[i].pos = next;
            }

            let target = self.bots[self.bots[i].target].pos;
            let b = &mut self.bots[i];
            let want = angle_to(b.pos, target);
            b.gun += norm(want - b.gun).clamp(-0.35, 0.35) * 0.3 * dt;
            b.radar += 0.3 * dt;
            b.heat -= 0.1 * dt;
            if b.heat <= 0.0 && norm(want - b.gun).abs() < 0.15 {
                let (pos, gun) = (b.pos, b.gun);
                let power = 1.0 + self.rand() * 2.0;
                let v = 20.0 - 3.0 * power;
                self.bullets.push(Bullet {
                    pos: pos + ahead(gun) * 20.0,
                    velocity: ahead(gun) * v * 0.5,
                    owner: i,
                    power,
                });
                self.bots[i].heat = 1.0 + power / 5.0 + self.rand() * 3.0;
            }
            if self.rand() < 0.003 {
                let skip = 1 + (self.rand() * (TANKS - 1) as f32) as usize;
                self.bots[i].target = (self.bots[i].target + skip) % TANKS;
            }
            if self.bots[i].target == i {
                self.bots[i].target = (i + 1) % TANKS;
            }
        }
        let area = Rect::from_min_size(Pos2::ZERO, self.size);
        let bots = &self.bots;
        let blasts = &mut self.blasts;
        self.bullets.retain_mut(|u| {
            u.pos += u.velocity * dt;
            let hit = bots.iter().enumerate().any(|(i, b)| {
                i != u.owner && (b.pos.x - u.pos.x).abs() < 18.0 && (b.pos.y - u.pos.y).abs() < 18.0
            });
            if hit {
                // Classic's scale for a bullet explosion.
                let scale = (1000.0 * u.power).sqrt() / 128.0;
                blasts.push(Blast {
                    pos: u.pos,
                    frame: 0.0,
                    scale,
                });
            }
            !hit && area.contains(u.pos)
        });
        for e in &mut self.blasts {
            e.frame += 0.4 * real;
        }
        self.blasts.retain(|e| e.frame < EXPLOSION_FRAMES);
    }

    /// Fills `rect` with the backdrop. `content` is the part the page covers: the tanks
    /// only move while the margins beside it are wide enough to see them.
    pub fn show(&mut self, ui: &egui::Ui, sprites: &mut Sprites, rect: Rect, content: Rect) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect);
        floor(&painter, sprites, rect);
        painter.rect_filled(rect, 0.0, SHADE);
        if self.size != rect.size() {
            self.size = rect.size();
            for b in &mut self.bots {
                b.pos = pos2(
                    b.pos.x.clamp(20.0, (self.size.x - 20.0).max(20.0)),
                    b.pos.y.clamp(20.0, (self.size.y - 20.0).max(20.0)),
                );
            }
        }
        if self.bots.is_empty() {
            self.spawn();
            // Some steps first, so the first frame already shows a battle going on.
            for _ in 0..90 {
                self.step(1.0, 1.0);
            }
        }
        let now = ctx.input(|i| i.time);
        let animate = content.left() - rect.left() >= MIN_MARGIN
            || rect.right() - content.right() >= MIN_MARGIN;
        if animate {
            let real = self
                .last
                .map_or(1.0, |t| ((now - t) * 60.0) as f32)
                .min(3.0);
            self.step(real * PACE, real);
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        self.last = Some(now);
        self.draw(&ctx, &painter, sprites, rect.min.to_vec2());
    }

    fn draw(&self, ctx: &egui::Context, painter: &egui::Painter, sprites: &mut Sprites, at: Vec2) {
        for b in &self.bots {
            painter.add(beam(b.pos + at, b.radar));
        }
        for b in &self.bots {
            for (part, color, heading) in [
                (Part::Body, b.colors[0], b.heading),
                (Part::Gun, b.colors[1], b.gun),
                (Part::Radar, b.colors[2], b.radar),
            ] {
                let (texture, size) = sprites.tank(ctx, part, color);
                painter.add(sprite(texture, b.pos + at, size, heading, Color32::WHITE));
            }
        }
        for u in &self.bullets {
            painter.circle_filled(u.pos + at, 1.5 + u.power * 0.6, BULLET);
        }
        for e in &self.blasts {
            if let Some(texture) = sprites.explosion.get(e.frame as usize) {
                let size = texture.size_vec2() * e.scale;
                painter.image(
                    texture.id(),
                    Rect::from_center_size(e.pos + at, size),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
    }
}

/// Classic's floor: one of its tiles per 64-pixel cell, from a fixed seed (as in the
/// battlefield view), anchored at the top left.
fn floor(painter: &egui::Painter, sprites: &Sprites, rect: Rect) {
    const CELL: f32 = 64.0;
    let mut seed: u32 = 20010101;
    let cols = (rect.width() / CELL).ceil().max(0.0) as usize;
    let rows = (rect.height() / CELL).ceil().max(0.0) as usize;
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    for row in 0..rows {
        for col in 0..cols {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let tile = ((f64::from(seed) / 4294967296.0) * 4.0).round() as usize;
            let min = rect.min + vec2(col as f32, row as f32) * CELL;
            let cell = Rect::from_min_size(min, Vec2::splat(CELL));
            painter.image(sprites.ground[tile].id(), cell, uv, Color32::WHITE);
        }
    }
}

/// The radar's beam: a narrow fan, fading out with distance.
fn beam(center: Pos2, radar: f32) -> egui::Shape {
    const LENGTH: f32 = 220.0;
    const HALF: f32 = 0.12;
    const STEPS: usize = 6;
    let mut mesh = Mesh::default();
    let near = Color32::from_rgba_unmultiplied(85, 239, 85, 46);
    mesh.colored_vertex(center, near);
    for k in 0..=STEPS {
        let a = radar - HALF + 2.0 * HALF * k as f32 / STEPS as f32;
        mesh.colored_vertex(center + ahead(a) * LENGTH, Color32::TRANSPARENT);
    }
    for k in 0..STEPS as u32 {
        mesh.add_triangle(0, k + 1, k + 2);
    }
    egui::Shape::mesh(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn angles_are_robocode_headings() {
        let o = pos2(100.0, 100.0);
        assert!(angle_to(o, pos2(100.0, 0.0)).abs() < 1e-6);
        assert!((angle_to(o, pos2(200.0, 100.0)) - PI / 2.0).abs() < 1e-6);
        assert!((ahead(PI / 2.0) - vec2(1.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn tanks_stay_inside() {
        let mut d = Backdrop {
            size: vec2(300.0, 200.0),
            ..Default::default()
        };
        d.spawn();
        for _ in 0..5000 {
            d.step(1.0, 1.0);
        }
        assert_eq!(d.bots.len(), TANKS);
        for b in &d.bots {
            assert!((20.0..=280.0).contains(&b.pos.x) && (20.0..=180.0).contains(&b.pos.y));
        }
    }
}
