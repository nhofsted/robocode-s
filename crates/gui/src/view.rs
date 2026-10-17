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
    /// How far a sentry robot's border reaches into the field (0: none known).
    pub sentry_border: f64,
}

/// How the battle is drawn (the options).
#[derive(Clone, Copy, Debug, Default)]
pub struct Style {
    /// Each robot's radar scan arc, as classic's "Visible scan arcs".
    pub scan_arcs: bool,
    /// Bullets in the robots' bullet colours; otherwise all in [`BULLET`].
    pub colored_bullets: bool,
    /// Each robot's name below it and its energy above it.
    pub robot_names: bool,
    pub robot_energy: bool,
    /// Explosions of bullet hits and of robots' deaths.
    pub explosions: bool,
    /// The floor's tiles; otherwise black.
    pub ground: bool,
    /// The sentry border, in a battle with a sentry robot.
    pub sentry_border: bool,
    /// The turns and frames per second, in the field's top left corner.
    pub rates: bool,
}

/// Classic's bullet colour when a robot sets none.
const BULLET: Color32 = Color32::from_gray(220);

/// How far the radar sees (`Rules.RADAR_SCAN_RADIUS`).
const RADAR_RANGE: f64 = 1200.0;

/// The most the view zooms in, relative to the whole battlefield fitting.
pub const MAX_ZOOM: f32 = 16.0;

/// Zoom and pan: how much of the battlefield the view shows, kept per battle or replay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// 1: the whole battlefield fits; up to [`MAX_ZOOM`].
    pub zoom: f32,
    /// The battlefield point (y up) at the centre of the view; `None`: the field's centre.
    pub center: Option<(f64, f64)>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: None,
        }
    }
}

impl Camera {
    /// The whole battlefield shows.
    pub fn fitted(&self) -> bool {
        self.zoom <= 1.0
    }

    /// The centre the view shows for a `width`×`height` field at `scale` (screen pixels
    /// per unit) in `available`: the camera's, moved so that the field fills the view
    /// along each axis where it is larger than the view, and centred where it is smaller.
    fn clamped_center(&self, available: Rect, width: f64, height: f64, scale: f32) -> (f64, f64) {
        let (x, y) = self.center.unwrap_or((width / 2.0, height / 2.0));
        let clamp = |v: f64, extent: f64, screen: f32| {
            let half = f64::from(screen / 2.0 / scale);
            if extent > 2.0 * half {
                v.clamp(half, extent - half)
            } else {
                extent / 2.0
            }
        };
        (
            clamp(x, width, available.width()),
            clamp(y, height, available.height()),
        )
    }

    /// Zooms by `factor`, keeping the battlefield point under `pointer` where it is.
    pub fn zoom_at(
        &mut self,
        factor: f32,
        pointer: Pos2,
        available: Rect,
        width: f64,
        height: f64,
    ) {
        let (before, _) = Transform::view(available, width, height, self);
        let zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        if before.scale == 0.0 || !zoom.is_finite() {
            return;
        }
        let (x, y) = before.field(pointer);
        let scale = before.scale / self.zoom.clamp(1.0, MAX_ZOOM) * zoom;
        let offset = pointer - available.center();
        self.zoom = zoom;
        self.center = Some((
            x - f64::from(offset.x / scale),
            y + f64::from(offset.y / scale),
        ));
        self.clamp(available, width, height);
    }

    /// Moves the battlefield by `delta` screen pixels.
    pub fn pan(&mut self, delta: Vec2, available: Rect, width: f64, height: f64) {
        let (t, _) = Transform::view(available, width, height, self);
        if t.scale == 0.0 {
            return;
        }
        let (x, y) = self.clamped_center(available, width, height, t.scale);
        self.center = Some((
            x - f64::from(delta.x / t.scale),
            y + f64::from(delta.y / t.scale),
        ));
        self.clamp(available, width, height);
    }

    /// Keeps the centre where the view can show it, so the field can't be lost.
    fn clamp(&mut self, available: Rect, width: f64, height: f64) {
        let (fit, _) = Transform::fit(available, width, height);
        if fit.scale > 0.0 {
            let scale = fit.scale * self.zoom;
            self.center = Some(self.clamped_center(available, width, height, scale));
        }
    }
}

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

    /// The battlefield as `camera` shows it in `available`: zoomed relative to
    /// [`Self::fit`] and moved to the camera's centre. The rectangle is the whole field's,
    /// which may reach past `available`.
    pub fn view(available: Rect, width: f64, height: f64, camera: &Camera) -> (Self, Rect) {
        let (fit, rect) = Self::fit(available, width, height);
        if fit.scale == 0.0 || camera.fitted() {
            return (fit, rect);
        }
        let scale = fit.scale * camera.zoom.min(MAX_ZOOM);
        let (x, y) = camera.clamped_center(available, width, height, scale);
        let c = available.center();
        let origin = pos2(c.x - x as f32 * scale, c.y - (height - y) as f32 * scale);
        let size = vec2(width as f32 * scale, height as f32 * scale);
        (
            Self {
                origin,
                scale,
                height,
            },
            Rect::from_min_size(origin, size),
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

    /// The battlefield point at a screen position (y up), unrounded; needs scale > 0.
    fn field(&self, p: Pos2) -> (f64, f64) {
        (
            f64::from((p.x - self.origin.x) / self.scale),
            self.height - f64::from((p.y - self.origin.y) / self.scale),
        )
    }

    /// The battlefield point at a screen position (y up), as classic's
    /// `InteractiveHandler` rounds it: `(int) (v + 0.5)`. None at scale 0.
    pub fn field_point(&self, p: Pos2) -> Option<(i32, i32)> {
        if self.scale == 0.0 {
            return None;
        }
        let (x, y) = self.field(p);
        Some(((x + 0.5) as i32, (y + 0.5) as i32))
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

/// What zooms and pans the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    /// Robots may read the mouse (a live battle: interactive robots): Ctrl+wheel zooms
    /// and Ctrl+drag pans; the plain wheel and drags are theirs.
    Modified,
    /// Nothing else reads the mouse (a replay): the plain wheel and drags too, and a
    /// double-click fits the field again.
    Free,
}

/// Zooms (around the pointer) and pans `camera` from this frame's input over
/// `available`, and shows a button to fit the whole field again while zoomed in.
pub fn navigate(
    ui: &mut egui::Ui,
    camera: &mut Camera,
    (width, height): (f64, f64),
    available: Rect,
    navigation: Navigation,
) {
    let id = ui.id().with("battlefield-camera");
    let response = ui.interact(available, id, egui::Sense::click_and_drag());
    let free = navigation == Navigation::Free;
    let (zoom, scroll, ctrl) =
        ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.y, i.modifiers.command));
    if let Some(pointer) = response.hover_pos() {
        // Ctrl+wheel (and a touchpad's pinch) comes as zoom, not scroll.
        let mut factor = zoom;
        if free {
            factor *= (scroll / 200.0).exp();
        }
        if factor != 1.0 {
            camera.zoom_at(factor, pointer, available, width, height);
        }
    }
    if response.dragged() && (free || ctrl) {
        camera.pan(response.drag_delta(), available, width, height);
    }
    if free && response.double_clicked() {
        *camera = Camera::default();
    }
    if camera.fitted() {
        zoom_hint(ui, &response, available, navigation);
        return;
    }
    let mut fit = false;
    egui::Area::new(ui.id().with("battlefield-fit"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(available.right_top() + vec2(-8.0, 8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(Color32::from_black_alpha(170))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", camera.zoom * 100.0))
                                .monospace()
                                .color(Color32::from_gray(200)),
                        );
                        let hint = if free {
                            "Fit the battlefield (or double-click it). The wheel zooms, a drag pans."
                        } else {
                            "Fit the battlefield. Ctrl+wheel zooms, Ctrl+drag pans."
                        };
                        let button = crate::icons::button(ui, 24.0, hint, false, |p, c, color| {
                            crate::icons::fit(p, c, 8.0, color)
                        });
                        fit = button.clicked();
                    });
                });
        });
    if fit {
        *camera = Camera::default();
    }
}

/// How long the zoom hint shows, in seconds of the pointer over the battlefield (once a
/// run, as the field can't tell you can zoom it).
const ZOOM_HINT: f64 = 4.0;

/// While the pointer is over the fitted battlefield, for the first [`ZOOM_HINT`] seconds
/// of a run: how to zoom, in its bottom right corner.
fn zoom_hint(ui: &egui::Ui, response: &egui::Response, available: Rect, navigation: Navigation) {
    let id = egui::Id::new("zoom-hint-shown");
    let (shown, dt) = (
        ui.ctx().data(|d| d.get_temp::<f64>(id)).unwrap_or(0.0),
        f64::from(ui.input(|i| i.stable_dt)).min(0.1),
    );
    if shown >= ZOOM_HINT || !response.hovered() {
        return;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, shown + dt));
    ui.ctx().request_repaint_after_secs(0.1);
    let text = match navigation {
        Navigation::Modified => "Ctrl+wheel zooms, Ctrl+drag pans",
        Navigation::Free => "The wheel zooms, a drag pans",
    };
    // Fades out over its last second.
    let alpha = (ZOOM_HINT - shown).clamp(0.0, 1.0) as f32;
    egui::Area::new(ui.id().with("zoom-hint"))
        .order(egui::Order::Middle)
        .interactable(false)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(available.right_bottom() + vec2(-8.0, -8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(Color32::from_black_alpha(170).gamma_multiply(alpha))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .size(12.0)
                            .color(Color32::from_gray(210).gamma_multiply(alpha)),
                    );
                });
        });
}

/// Draws one turn into `available` as `camera` shows it; returns the transform and the
/// battlefield's visible screen rectangle. `previous` is the turn before, for the scan arcs.
pub fn draw(
    ui: &mut egui::Ui,
    sprites: &mut Sprites,
    field: &Field,
    style: Style,
    (previous, snapshot): (Option<&v1::TurnSnapshot>, Option<&v1::TurnSnapshot>),
    available: Rect,
    camera: &Camera,
) -> (Transform, Rect) {
    let (t, whole) = Transform::view(available, field.width, field.height, camera);
    let rect = whole.intersect(available);
    let painter = ui.painter_at(available);
    painter.rect_filled(available, 0.0, Color32::BLACK);
    if t.scale == 0.0 {
        return (t, rect);
    }
    if style.ground {
        ground(&painter, sprites, t, field);
    } else {
        // Told apart from the black around it.
        painter.rect_filled(rect, 0.0, Color32::from_gray(18));
    }
    if style.rates {
        rates(ui, &painter, rect, snapshot);
    }
    let Some(snapshot) = snapshot else {
        return (t, rect);
    };
    if style.sentry_border
        && field.sentry_border > 0.0
        && field
            .robots
            .iter()
            .any(|r| r.r#type.is_some_and(|t| t.sentry))
    {
        sentry_border(&painter, t, field);
    }
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
        if style.robot_energy {
            painter.text(
                center - vec2(0.0, half),
                egui::Align2::CENTER_BOTTOM,
                format!("{:.1}", robot.energy.unwrap_or_default()),
                font.clone(),
                Color32::WHITE,
            );
        }
        if style.robot_names {
            painter.text(
                center + vec2(0.0, half),
                egui::Align2::CENTER_TOP,
                &statics.name,
                font,
                Color32::WHITE,
            );
        }
    }
    for b in &snapshot.bullets {
        bullet(&painter, sprites, t, field, style, snapshot, b);
    }
    (t, rect)
}

/// The sentry border: a translucent red band along the walls, as wide as the border
/// reaches into the field.
fn sentry_border(painter: &egui::Painter, t: Transform, field: &Field) {
    let (w, h) = (field.width, field.height);
    let b = field.sentry_border.min(w / 2.0).min(h / 2.0);
    let color = Color32::from_rgba_unmultiplied(255, 0, 0, 32);
    let band = |x0: f64, y0: f64, x1: f64, y1: f64| {
        painter.rect_filled(
            Rect::from_two_pos(t.point(x0, y0), t.point(x1, y1)),
            0.0,
            color,
        );
    };
    band(0.0, 0.0, w, b);
    band(0.0, h - b, w, h);
    band(0.0, b, b, h - b);
    band(w - b, b, w, h - b);
}

/// The turns and frames per second over the last second, measured here (once a frame),
/// in the top left corner of `rect`.
fn rates(ui: &egui::Ui, painter: &egui::Painter, rect: Rect, snapshot: Option<&v1::TurnSnapshot>) {
    /// Since the last update: when it started, the frames and turns since, the last turn
    /// seen; and the rates shown.
    #[derive(Clone, Copy, Default)]
    struct Rates {
        since: f64,
        frames: u32,
        turns: u32,
        last: Option<(i32, i64)>,
        shown: (f64, f64),
    }
    let id = egui::Id::new("view-rates");
    let now = ui.input(|i| i.time);
    let mut r: Rates = ui.data(|d| d.get_temp(id)).unwrap_or(Rates {
        since: now,
        ..Default::default()
    });
    r.frames += 1;
    let turn = snapshot.map(|s| (s.round, s.turn));
    if turn.is_some() && turn != r.last {
        r.turns += 1;
        r.last = turn;
    }
    let elapsed = now - r.since;
    if elapsed >= 1.0 {
        r.shown = (f64::from(r.turns) / elapsed, f64::from(r.frames) / elapsed);
        (r.since, r.frames, r.turns) = (now, 0, 0);
    }
    ui.data_mut(|d| d.insert_temp(id, r));
    let text = format!("TPS {:.0}  FPS {:.0}", r.shown.0, r.shown.1);
    let font = egui::FontId::monospace(11.0);
    let at = rect.left_top() + vec2(6.0, 4.0);
    painter.text(
        at + vec2(1.0, 1.0),
        egui::Align2::LEFT_TOP,
        &text,
        font.clone(),
        Color32::BLACK,
    );
    painter.text(at, egui::Align2::LEFT_TOP, text, font, Color32::WHITE);
    // Keep measuring while nothing else repaints.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(250));
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
            if !style.explosions {
                return;
            }
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
    fn field_points_are_mirrored_and_rounded_as_classic() {
        let available = Rect::from_min_size(pos2(10.0, 20.0), vec2(1600.0, 1200.0));
        let (t, _) = Transform::fit(available, 800.0, 600.0);
        // Scale 2: the bottom left corner is (0, 0), y grows upwards.
        assert_eq!(t.field_point(pos2(10.0, 1220.0)), Some((0, 0)));
        assert_eq!(t.field_point(pos2(210.0, 220.0)), Some((100, 500)));
        assert_eq!(t.field_point(pos2(211.0, 219.0)), Some((101, 501)));
        assert_eq!(t.field_point(t.point(123.0, 456.0)), Some((123, 456)));
        let (none, _) = Transform::fit(Rect::NOTHING, 800.0, 600.0);
        assert_eq!(none.field_point(pos2(1.0, 1.0)), None);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let available = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let mut camera = Camera::default();
        let pointer = pos2(200.0, 150.0);
        let (t, _) = Transform::view(available, 800.0, 600.0, &camera);
        let before = t.field(pointer);
        camera.zoom_at(2.0, pointer, available, 800.0, 600.0);
        let (t, rect) = Transform::view(available, 800.0, 600.0, &camera);
        assert_eq!(t.scale, 2.0);
        assert_eq!(rect.size(), vec2(1600.0, 1200.0));
        let after = t.field(pointer);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
        // Interactive robots get the same point.
        assert_eq!(t.field_point(t.point(250.0, 400.0)), Some((250, 400)));
        // Zooming out all the way fits again; no further than that.
        camera.zoom_at(0.01, pointer, available, 800.0, 600.0);
        assert!(camera.fitted());
        assert_eq!(
            Transform::view(available, 800.0, 600.0, &camera).0.scale,
            1.0
        );
        camera.zoom_at(1000.0, pointer, available, 800.0, 600.0);
        assert_eq!(camera.zoom, MAX_ZOOM);
    }

    #[test]
    fn pan_cannot_lose_the_field() {
        let available = Rect::from_min_size(pos2(10.0, 20.0), vec2(1000.0, 600.0));
        let mut camera = Camera {
            zoom: 2.0,
            center: None,
        };
        // Scale 2: the view shows 500×300 of the 800×600 field.
        camera.pan(vec2(-100.0, 40.0), available, 800.0, 600.0);
        assert_eq!(camera.center, Some((450.0, 320.0)));
        camera.pan(vec2(1e6, -1e6), available, 800.0, 600.0);
        assert_eq!(camera.center, Some((250.0, 150.0)));
        let (t, rect) = Transform::view(available, 800.0, 600.0, &camera);
        assert!(rect.contains_rect(available));
        assert_eq!(t.point(0.0, 0.0), available.left_bottom());
        // Wider than the field at this zoom: centred across.
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(3000.0, 600.0));
        camera.pan(vec2(500.0, 0.0), wide, 800.0, 600.0);
        assert_eq!(camera.center.unwrap().0, 400.0);
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
