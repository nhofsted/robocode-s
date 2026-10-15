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
