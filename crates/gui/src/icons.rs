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

/// A square, borderless button showing an icon; brighter when hovered.
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
    let color = if selected || response.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    draw(ui.painter(), rect.center(), color);
    response
}
