// SPDX-License-Identifier: EPL-2.0

//! The app's shell, Steam-like: a top bar with the tabs on the left and the options gear on
//! the right (in the title bar when we draw the window frame ourselves), a footer with the
//! version, and pages as a width-limited column centred over the backdrop.

use crate::backdrop::Backdrop;
use crate::icons;
use crate::sprites::{Part, Sprites};
use crate::view::sprite;
use crate::window::{self, BAR_HEIGHT};
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, vec2};

/// The website's colours (`web/src/app.css`): background, panel, line and radar green.
pub const BAR: Color32 = Color32::from_rgb(0x0a, 0x0e, 0x15);
pub const PANEL: Color32 = Color32::from_rgb(0x10, 0x16, 0x1f);
pub const LINE: Color32 = Color32::from_rgb(0x24, 0x32, 0x47);
pub const ACCENT: Color32 = Color32::from_rgb(0x55, 0xef, 0x55);

/// The widest a page's column gets, and the least floor shown beside it.
const COLUMN: f32 = 960.0;
const MARGIN: f32 = 16.0;
const PADDING: i8 = 24;
const FOOTER_HEIGHT: f32 = 22.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Start,
    Watch,
    Rumble,
    Options,
}

impl Tab {
    /// The tabs in the top bar; [`Tab::Options`] is behind the gear.
    const BAR: [Tab; 3] = [Tab::Start, Tab::Watch, Tab::Rumble];

    fn label(self) -> &'static str {
        match self {
            Tab::Start => "Start Battle",
            Tab::Watch => "Watch Battle",
            Tab::Rumble => "Run Rumble",
            Tab::Options => "Options",
        }
    }

    /// `--tab`: a tab by name (`replay`, the Watch Battle tab's old name, still works).
    pub fn parse(name: &str) -> Option<Tab> {
        match name {
            "start" => Some(Tab::Start),
            "watch" | "replay" => Some(Tab::Watch),
            "options" => Some(Tab::Options),
            _ => None,
        }
    }

    /// Why a tab can't be opened yet.
    fn disabled(self) -> Option<&'static str> {
        match self {
            Tab::Rumble => Some("Coming with the rumble client (M5)"),
            _ => None,
        }
    }
}

/// The top bar: the app's name, the tabs and the gear. With our own window frame it is the
/// title bar (see [`window::show`]); call it before the other panels.
pub fn top_bar(root: &mut egui::Ui, own_frame: bool, sprites: &mut Sprites, tab: &mut Tab) {
    let content = |ui: &mut egui::Ui| {
        ui.horizontal_centered(|ui| {
            ui.add_space(12.0);
            logo(ui, sprites);
            ui.add_space(20.0);
            for t in Tab::BAR {
                if tab_button(ui, t, *tab == t).clicked() {
                    *tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let options = *tab == Tab::Options;
                let gear = icons::button(ui, 32.0, "Options", options, |p, c, color| {
                    icons::gear(p, c, 9.0, color);
                });
                if gear.clicked() {
                    *tab = if options { Tab::Start } else { Tab::Options };
                }
            });
        });
    };
    if own_frame {
        window::show(root, BAR, content);
    } else {
        egui::Panel::top("top-bar")
            .exact_size(BAR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(BAR))
            .show(root, content);
    }
}

/// A small tank and the name, as on the website's header.
fn logo(ui: &mut egui::Ui, sprites: &mut Sprites) {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    let ctx = ui.ctx().clone();
    let colors = [[63, 138, 79], [159, 212, 168], [255, 215, 94]];
    for (part, color) in [Part::Body, Part::Gun, Part::Radar].into_iter().zip(colors) {
        let (texture, size) = sprites.tank(&ctx, part, color);
        let shape = sprite(texture, rect.center(), size * 0.5, 0.6, Color32::WHITE);
        ui.painter().add(shape);
    }
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("robocode-s")
            .font(FontId::proportional(16.0))
            .strong()
            .color(ui.visuals().strong_text_color()),
    );
}

/// A tab: its name, underlined in radar green when it is open.
fn tab_button(ui: &mut egui::Ui, tab: Tab, open: bool) -> egui::Response {
    let font = FontId::proportional(14.0);
    let galley =
        ui.painter()
            .layout_no_wrap(tab.label().to_uppercase(), font, Color32::PLACEHOLDER);
    let size = vec2(galley.size().x + 28.0, BAR_HEIGHT);
    let enabled = tab.disabled().is_none();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let visuals = ui.visuals();
    let color = if !enabled {
        visuals.weak_text_color().gamma_multiply(0.5)
    } else if open || response.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    let text = Align2::CENTER_CENTER
        .anchor_size(rect.center(), galley.size())
        .min;
    ui.painter().galley(text, galley, color);
    if open {
        let line = Rect::from_min_max(
            rect.left_bottom() + vec2(10.0, -3.0),
            rect.right_bottom() + vec2(-10.0, 0.0),
        );
        ui.painter().rect_filled(line, 1.0, ACCENT);
    }
    match tab.disabled() {
        Some(why) => response.on_hover_text(why),
        None => response.on_hover_cursor(egui::CursorIcon::PointingHand),
    }
}

/// The footer: the version, bottom right. Call it before the page's panels.
pub fn footer(root: &mut egui::Ui) {
    egui::Panel::bottom("footer")
        .exact_size(FOOTER_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(BAR))
        .show(root, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(concat!("robocode-s ", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                );
            });
        });
}

/// Where a page's column goes in `area`: at most [`COLUMN`] wide, centred, with at least
/// [`MARGIN`] of floor around it.
fn column(area: Rect) -> Rect {
    let width = (area.width() - 2.0 * MARGIN).clamp(0.0, COLUMN);
    let x = area.center().x - width / 2.0;
    Rect::from_min_max(
        egui::pos2(x, area.top() + MARGIN),
        egui::pos2(x + width, (area.bottom() - MARGIN).max(area.top() + MARGIN)),
    )
}

/// A page: the backdrop fills the rest of the window, and `content` goes in a centred
/// column that is as tall as it needs (scrolling when the window is too short). `state` is
/// what both draw from (the app), handed to each in turn.
pub fn page<T>(
    root: &mut egui::Ui,
    state: &mut T,
    scenery: impl FnOnce(&mut T) -> (&mut Backdrop, &mut Sprites),
    content: impl FnOnce(&mut T, &mut egui::Ui),
) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(root, |ui| {
            let area = ui.max_rect();
            let column = column(area);
            let (backdrop, sprites) = scenery(state);
            backdrop.show(ui, sprites, area, column);
            ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                egui::Frame::NONE
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0, LINE))
                    .corner_radius(4.0)
                    .inner_margin(PADDING)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, true])
                            .max_height(column.height() - 2.0 * f32::from(PADDING))
                            .show(ui, |ui| content(state, ui));
                    });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn tabs_by_name_with_the_old_replay_name() {
        assert_eq!(Tab::parse("watch"), Some(Tab::Watch));
        assert_eq!(Tab::parse("replay"), Some(Tab::Watch));
        assert_eq!(Tab::parse("start"), Some(Tab::Start));
        assert_eq!(Tab::parse("rumble"), None);
    }

    #[test]
    fn the_column_is_centred_with_adaptive_margins() {
        let wide = column(Rect::from_min_max(pos2(0.0, 40.0), pos2(1600.0, 900.0)));
        assert_eq!(wide.width(), COLUMN);
        assert_eq!(wide.center().x, 800.0);
        let narrow = column(Rect::from_min_max(pos2(0.0, 40.0), pos2(800.0, 600.0)));
        assert_eq!((narrow.left(), narrow.right()), (MARGIN, 800.0 - MARGIN));
        let none = column(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)));
        assert!(none.width() >= 0.0 && none.height() >= 0.0);
    }
}
