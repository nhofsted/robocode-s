// SPDX-License-Identifier: EPL-2.0

//! Icons drawn with the painter: egui's default fonts lack many symbols (`✕` showed as a
//! box), and drawn shapes stay crisp at any scale.

use eframe::egui::{self, Color32, Pos2, Sense, Shape, Stroke, vec2};

/// A gear: a ring with eight teeth, `radius` to the tips of the teeth.
pub fn gear(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    const TEETH: usize = 8;
    let ring = radius * 0.68;
    let width = radius * 0.3;
    painter.circle_stroke(center, ring - width / 2.0, Stroke::new(width, color));
    for k in 0..TEETH {
        let a = k as f32 * std::f32::consts::TAU / TEETH as f32;
        let (along, across) = (vec2(a.sin(), -a.cos()), vec2(a.cos(), a.sin()));
        // A tooth narrows a little towards its tip.
        let (base, tip) = (radius * 0.2, radius * 0.15);
        let inner = center + along * (ring - 1.0);
        let outer = center + along * radius;
        painter.add(Shape::convex_polygon(
            vec![
                inner - across * base,
                outer - across * tip,
                outer + across * tip,
                inner + across * base,
            ],
            color,
            Stroke::NONE,
        ));
    }
}

/// A cross (remove, close): two strokes, `radius` from the centre to each end.
pub fn cross(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new((radius * 0.35).max(1.5), color);
    let (a, b) = (vec2(radius, radius), vec2(radius, -radius));
    painter.line_segment([center - a, center + a], stroke);
    painter.line_segment([center - b, center + b], stroke);
}

/// A plus (add): two strokes, `radius` from the centre to each end.
pub fn plus(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new((radius * 0.35).max(1.5), color);
    let (a, b) = (vec2(radius, 0.0), vec2(0.0, radius));
    painter.line_segment([center - a, center + a], stroke);
    painter.line_segment([center - b, center + b], stroke);
}

/// Play: a triangle pointing right, `radius` from the centre to its tip.
pub fn play(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    triangle(
        painter,
        center + vec2(radius * 0.15, 0.0),
        radius,
        1.0,
        color,
    );
}

/// A triangle pointing right (`dir` 1) or left (-1), `radius` from its centre to its tip.
fn triangle(painter: &egui::Painter, center: Pos2, radius: f32, dir: f32, color: Color32) {
    let (back, half) = (radius * 0.75, radius * 0.95);
    painter.add(Shape::convex_polygon(
        vec![
            center + vec2(-back * dir, -half),
            center + vec2(radius * dir, 0.0),
            center + vec2(-back * dir, half),
        ],
        color,
        Stroke::NONE,
    ));
}

/// Pause: two bars.
pub fn pause(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let (w, h) = (radius * 0.55, radius * 1.7);
    for dx in [-radius * 0.5, radius * 0.5] {
        let bar = egui::Rect::from_center_size(center + vec2(dx, 0.0), vec2(w, h));
        painter.rect_filled(bar, 1.0, color);
    }
}

/// Stop: a square.
pub fn stop(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let square = egui::Rect::from_center_size(center, vec2(radius * 1.6, radius * 1.6));
    painter.rect_filled(square, 1.5, color);
}

/// One step forward (`dir` 1) or back (-1): a triangle against a bar.
pub fn step(painter: &egui::Painter, center: Pos2, radius: f32, dir: f32, color: Color32) {
    let tip = radius * 0.8;
    triangle(
        painter,
        center + vec2(-radius * 0.2 * dir, 0.0),
        tip,
        dir,
        color,
    );
    let bar = egui::Rect::from_center_size(
        center + vec2(radius * 0.8 * dir, 0.0),
        vec2(radius * 0.4, radius * 1.6),
    );
    painter.rect_filled(bar, 1.0, color);
}

/// Speed: two triangles, as a fast-forward button.
pub fn speed(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let r = radius * 0.6;
    triangle(painter, center + vec2(-r * 0.75, 0.0), r, 1.0, color);
    triangle(painter, center + vec2(r * 0.95, 0.0), r, 1.0, color);
}

/// Debug paint: a pencil.
pub fn pencil(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    // Along the pencil, from its tip (bottom left) to its end (top right), and across it.
    let along = vec2(1.0, -1.0).normalized();
    let across = vec2(along.y, -along.x);
    let (half, w) = (radius * 1.05, radius * 0.32);
    let tip = center - along * half;
    let shoulder = tip + along * radius * 0.6;
    let end = center + along * half;
    painter.add(Shape::convex_polygon(
        vec![
            shoulder - across * w,
            end - across * w,
            end + across * w,
            shoulder + across * w,
        ],
        color,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![shoulder - across * w, shoulder + across * w, tip],
        color,
        Stroke::NONE,
    ));
}

/// Browse: a folder with its tab on the top left.
pub fn folder(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let (w, h) = (radius * 1.1, radius * 0.75);
    let (l, r, t, b) = (center.x - w, center.x + w, center.y - h * 0.7, center.y + h);
    let tab = radius * 0.35;
    let stroke = Stroke::new(1.5, color);
    painter.add(Shape::closed_line(
        vec![
            egui::pos2(l, t - tab),
            egui::pos2(l + w * 0.7, t - tab),
            egui::pos2(l + w * 0.7 + tab, t),
            egui::pos2(r, t),
            egui::pos2(r, b),
            egui::pos2(l, b),
        ],
        stroke,
    ));
    painter.line_segment([egui::pos2(l, t + tab), egui::pos2(r, t + tab)], stroke);
}

/// Copy: two overlapping pages, the front one bottom right.
pub fn copy(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let (w, h) = (radius * 0.6, radius * 0.75);
    let shift = vec2(radius * 0.25, radius * 0.25);
    let back = egui::Rect::from_center_size(center - shift, vec2(w * 2.0, h * 2.0));
    let front = egui::Rect::from_center_size(center + shift, vec2(w * 2.0, h * 2.0));
    // The back page shows only where the front one doesn't cover it.
    painter.line_segment([back.left_bottom(), back.left_top()], stroke);
    painter.line_segment([back.left_top(), back.right_top()], stroke);
    painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
}

/// Save: an arrow down into a tray.
pub fn save(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let top = center.y - radius;
    let tip = center.y + radius * 0.35;
    painter.line_segment(
        [egui::pos2(center.x, top), egui::pos2(center.x, tip)],
        stroke,
    );
    let head = radius * 0.45;
    painter.line_segment(
        [
            egui::pos2(center.x - head, tip - head),
            egui::pos2(center.x, tip),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x + head, tip - head),
            egui::pos2(center.x, tip),
        ],
        stroke,
    );
    let (l, r, b) = (center.x - radius, center.x + radius, center.y + radius);
    painter.add(Shape::line(
        vec![
            egui::pos2(l, center.y + radius * 0.3),
            egui::pos2(l, b),
            egui::pos2(r, b),
            egui::pos2(r, center.y + radius * 0.3),
        ],
        stroke,
    ));
}

/// Clear: a bin with a lid.
pub fn clear(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let lid = center.y - radius * 0.65;
    let (top, bottom) = (lid + radius * 0.3, center.y + radius);
    let (w, narrow) = (radius * 0.7, radius * 0.55);
    painter.line_segment(
        [
            egui::pos2(center.x - radius, lid),
            egui::pos2(center.x + radius, lid),
        ],
        stroke,
    );
    let handle = radius * 0.3;
    painter.line_segment(
        [
            egui::pos2(center.x - handle, lid - radius * 0.3),
            egui::pos2(center.x + handle, lid - radius * 0.3),
        ],
        stroke,
    );
    painter.add(Shape::line(
        vec![
            egui::pos2(center.x - w, top),
            egui::pos2(center.x - narrow, bottom),
            egui::pos2(center.x + narrow, bottom),
            egui::pos2(center.x + w, top),
        ],
        stroke,
    ));
}

/// Kill (a robot): crosshairs, a ring with four ticks reaching in and a dot at the centre.
pub fn crosshairs(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let ring = radius * 0.7;
    painter.circle_stroke(center, ring, stroke);
    for d in [
        vec2(1.0, 0.0),
        vec2(-1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(0.0, -1.0),
    ] {
        painter.line_segment(
            [center + d * radius * 1.1, center + d * ring * 0.45],
            stroke,
        );
    }
    painter.circle_filled(center, 1.2, color);
}

/// Fit (the whole battlefield): four corner brackets, `radius` from the centre to each
/// corner along both axes.
pub fn fit(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let arm = radius * 0.45;
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let corner = center + vec2(sx * radius, sy * radius);
        painter.add(Shape::line(
            vec![
                corner - vec2(0.0, sy * arm),
                corner,
                corner - vec2(sx * arm, 0.0),
            ],
            stroke,
        ));
    }
    painter.rect_stroke(
        egui::Rect::from_center_size(center, vec2(radius, radius * 0.75)),
        1.0,
        stroke,
        egui::StrokeKind::Middle,
    );
}

/// A square, borderless button showing an icon; brighter when hovered, dimmed when the
/// `ui` is disabled.
pub fn button(
    ui: &mut egui::Ui,
    size: f32,
    hint: &str,
    selected: bool,
    draw: impl FnOnce(&egui::Painter, Pos2, Color32),
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let response = response.on_hover_text(hint);
    let visuals = ui.visuals();
    let color = if !ui.is_enabled() {
        visuals.weak_text_color().gamma_multiply(0.4)
    } else if selected || response.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    if response.hovered() && ui.is_enabled() {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    draw(ui.painter(), rect.center(), color);
    response
}
