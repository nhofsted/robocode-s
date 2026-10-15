// SPDX-License-Identifier: EPL-2.0

//! Our own window frame (title bar, buttons, resize edges) where the system's crashes.
//!
//! On WSLg the compositor (Weston 9 with its pixman renderer) draws no decorations, so
//! winit draws its own frame from a `wl_shm` pool that it grows as the window gets wider.
//! Weston segfaults in pixman when that pool is resized while it holds the old buffer,
//! which takes the app down with it: dragging the window wider crashed it (#28). Without
//! winit's frame there is no pool to resize; egui draws the frame instead.

use eframe::egui::{self, Color32, CursorIcon, Id, PointerButton, Sense, Stroke, vec2};
use egui::viewport::{ResizeDirection, ViewportCommand};

/// Width of the resize edges, in points.
const EDGE: f32 = 5.0;
/// Length of the corner zones along each edge.
const CORNER: f32 = 16.0;
const TITLE_HEIGHT: f32 = 28.0;

/// Whether to draw our own frame: on WSLg under Wayland, unless `ROBOCODES_OWN_FRAME`
/// says otherwise (`1` or `0`).
pub fn wanted() -> bool {
    match std::env::var("ROBOCODES_OWN_FRAME").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => {
            std::env::var_os("WAYLAND_DISPLAY").is_some()
                && std::path::Path::new("/mnt/wslg").is_dir()
        }
    }
}

/// Draws the frame around the window's content; call it before the other panels.
pub fn show(root: &mut egui::Ui, title: &str) {
    let ctx = root.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    if !maximized {
        edges(root);
    }
    egui::Panel::top("window-title")
        .exact_size(TITLE_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(root.visuals().extreme_bg_color))
        .show(root, |ui| {
            let rect = ui.max_rect();
            let bar = ui.interact(rect, Id::new("window-title-bar"), Sense::click_and_drag());
            if bar.drag_started_by(PointerButton::Primary) {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
            ui.painter().text(
                rect.left_center() + vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if button(ui, Icon::Close, "Close").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    let (icon, hint) = if maximized {
                        (Icon::Restore, "Restore")
                    } else {
                        (Icon::Maximize, "Maximize")
                    };
                    if button(ui, icon, hint).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                    }
                    if button(ui, Icon::Minimize, "Minimize").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    }
                });
            });
        });
}

/// Thin panels along the four sides that start a resize when pressed.
fn edges(root: &mut egui::Ui) {
    let fill = root.visuals().extreme_bg_color;
    let frame = egui::Frame::NONE.fill(fill);
    let sides = [
        egui::Panel::top("window-edge-n"),
        egui::Panel::bottom("window-edge-s"),
        egui::Panel::left("window-edge-w"),
        egui::Panel::right("window-edge-e"),
    ];
    for panel in sides {
        panel
            .exact_size(EDGE)
            .resizable(false)
            .show_separator_line(false)
            .frame(frame)
            .show(root, |ui| {
                let rect = ui.max_rect();
                let window = ui.ctx().content_rect();
                let response = ui.interact(rect, ui.id().with("grip"), Sense::drag());
                let Some(pos) = response.hover_pos().or(response.interact_pointer_pos()) else {
                    return;
                };
                let direction = direction(window, pos);
                let response = response.on_hover_cursor(cursor(direction));
                let pressed = ui.input(|i| i.pointer.primary_pressed());
                if pressed && response.is_pointer_button_down_on() {
                    ui.ctx()
                        .send_viewport_cmd(ViewportCommand::BeginResize(direction));
                }
            });
    }
}

/// Which way to resize when the pointer is at `pos` on the window's edge: the nearest
/// side, or a corner within [`CORNER`] of it.
fn direction(window: egui::Rect, pos: egui::Pos2) -> ResizeDirection {
    let west = pos.x - window.left() < CORNER;
    let east = window.right() - pos.x < CORNER;
    let north = pos.y - window.top() < CORNER;
    let south = window.bottom() - pos.y < CORNER;
    match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        _ => {
            // Not in a corner: the side the pointer is closest to.
            let sides = [
                (pos.y - window.top(), ResizeDirection::North),
                (window.bottom() - pos.y, ResizeDirection::South),
                (pos.x - window.left(), ResizeDirection::West),
                (window.right() - pos.x, ResizeDirection::East),
            ];
            sides
                .into_iter()
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map_or(ResizeDirection::SouthEast, |(_, d)| d)
        }
    }
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A title-bar button with a drawn icon (the default font lacks some symbols).
fn button(ui: &mut egui::Ui, icon: Icon, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, TITLE_HEIGHT), Sense::click());
    let response = response.on_hover_text(hint);
    let painter = ui.painter();
    if response.hovered() {
        let fill = match icon {
            Icon::Close => Color32::from_rgb(196, 43, 28),
            _ => ui.visuals().widgets.hovered.weak_bg_fill,
        };
        painter.rect_filled(rect, 0.0, fill);
    }
    let stroke = Stroke::new(1.0, ui.visuals().text_color());
    let c = rect.center();
    let r = 5.0;
    match icon {
        Icon::Minimize => {
            painter.line_segment([c + vec2(-r, 0.0), c + vec2(r, 0.0)], stroke);
        }
        Icon::Maximize => {
            let square = egui::Rect::from_center_size(c, egui::Vec2::splat(2.0 * r));
            painter.rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
        }
        Icon::Restore => {
            let back =
                egui::Rect::from_center_size(c + vec2(2.0, -2.0), egui::Vec2::splat(1.6 * r));
            let front = back.translate(vec2(-4.0, 4.0));
            painter.rect_stroke(back, 0.0, stroke, egui::StrokeKind::Inside);
            painter.rect_filled(front, 0.0, ui.visuals().extreme_bg_color);
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        Icon::Close => {
            painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
            painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2};

    #[test]
    fn corners_and_sides() {
        let w = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));
        assert_eq!(direction(w, pos2(798.0, 598.0)), ResizeDirection::SouthEast);
        assert_eq!(direction(w, pos2(790.0, 598.0)), ResizeDirection::SouthEast);
        assert_eq!(direction(w, pos2(2.0, 2.0)), ResizeDirection::NorthWest);
        assert_eq!(direction(w, pos2(798.0, 2.0)), ResizeDirection::NorthEast);
        assert_eq!(direction(w, pos2(2.0, 598.0)), ResizeDirection::SouthWest);
        assert_eq!(direction(w, pos2(400.0, 598.0)), ResizeDirection::South);
        assert_eq!(direction(w, pos2(400.0, 2.0)), ResizeDirection::North);
        assert_eq!(direction(w, pos2(2.0, 300.0)), ResizeDirection::West);
        assert_eq!(direction(w, pos2(798.0, 300.0)), ResizeDirection::East);
    }
}
