// SPDX-License-Identifier: EPL-2.0

//! The battle screen, shared by a live battle and a replay: the battlefield fills the tab,
//! with the controls below it, the robots' output in tabs below those, and the robots on
//! the right, running the full height, with their graphs below them (#33).
//!
//! Add the panels in this order: [`robots_panel`], [`output_panel`], [`controls_panel`],
//! then the field in the central panel.

use crate::graphs::{self, History};
use crate::icons;
use crate::shell;
use crate::sprites::{Part, Sprites, default_colors};
use crate::view;
use eframe::egui::{self, Color32, FontId, Rect, RichText, Sense, Stroke, pos2, vec2};
use robocode_s_protocol::v1;
use robocode_s_server::results::{COLUMNS, Contestant, cells, contestants, round};
use std::borrow::Cow;

/// The size of a control's icon button.
const CONTROL: f32 = 30.0;
/// The fastest turn rate the speed slider sets before "max".
const MAX_TPS: u32 = 1000;
/// Energy bars: the colours above half, low and very low energy.
const ENERGY: Color32 = Color32::from_rgb(0x3f, 0x8a, 0x4f);
const ENERGY_LOW: Color32 = Color32::from_rgb(0xc9, 0x8a, 0x1c);
const ENERGY_CRITICAL: Color32 = Color32::from_rgb(0xc0, 0x3a, 0x3a);

/// The right side: per robot its colour, name, score and energy, and below them the
/// graphs of the first `shown` turns of `history`. A click on a robot opens its output tab.
/// Without `scores` (a viewer track, which has no scores, CPU times or other rounds), only
/// names, energy and the energy graph.
#[allow(clippy::too_many_arguments)]
pub fn robots_panel(
    root: &mut egui::Ui,
    id: &str,
    robots: &[v1::RobotStatics],
    snapshot: Option<&v1::TurnSnapshot>,
    history: &History,
    shown: usize,
    selected: &mut usize,
    scores: bool,
) {
    // A narrow window keeps most of its width for the field.
    let width = (root.available_width() * 0.22).clamp(180.0, 260.0);
    egui::Panel::right(id.to_string())
        .default_size(width)
        .size_range(180.0..=420.0)
        .frame(
            egui::Frame::NONE
                .fill(shell::PANEL)
                .inner_margin(egui::Margin::symmetric(10, 10)),
        )
        .show(root, |ui| {
            ui.label(
                RichText::new("ROBOTS")
                    .size(12.0)
                    .strong()
                    .color(shell::ACCENT),
            );
            ui.add_space(4.0);
            // The graphs take the bottom, the robots scroll above them.
            let graphs = (ui.available_height() * 0.42).clamp(150.0, 340.0);
            let round = snapshot.map_or(0, |s| s.round);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(
                    (ui.available_height() - graphs - 12.0 - 3.0 * ui.spacing().item_spacing.y)
                        .max(48.0),
                )
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, statics) in robots.iter().enumerate() {
                        let robot = snapshot.and_then(|s| s.robots.get(i));
                        let score =
                            scores.then(|| robot.map_or(0.0, |r| history.score(round, i, r)));
                        if robot_row(ui, statics, robot, score, *selected == i).clicked() {
                            *selected = i;
                        }
                    }
                });
            ui.add_space(12.0);
            graphs::graphs(ui, robots, snapshot, history, shown, graphs, scores);
        });
}

/// A robot in the list: its colour, name and score (if any) over its energy bar.
fn robot_row(
    ui: &mut egui::Ui,
    statics: &v1::RobotStatics,
    robot: Option<&v1::RobotSnapshot>,
    score: Option<f64>,
    selected: bool,
) -> egui::Response {
    const HEIGHT: f32 = 44.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::click());
    let response = response.on_hover_text(format!("{}: show its output", statics.name));
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    if selected || response.hovered() {
        let fill = if selected {
            visuals.selection.bg_fill.gamma_multiply(0.45)
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(rect, 3.0, fill);
    }
    let inner = rect.shrink2(vec2(6.0, 5.0));
    let dead = robot.is_none_or(|r| r.state == v1::RobotState::Dead as i32);
    let energy = robot.and_then(|r| r.energy).unwrap_or_default();
    // The first line: colour, name and score.
    let line = inner.top() + 8.0;
    swatch(&painter, pos2(inner.left() + 5.0, line), statics, robot);
    let right = match score {
        Some(score) => {
            painter
                .text(
                    pos2(inner.right(), line),
                    egui::Align2::RIGHT_CENTER,
                    format!("{score:.0}"),
                    FontId::proportional(13.0),
                    visuals.text_color(),
                )
                .left()
                - 8.0
        }
        None => inner.right(),
    };
    let color = if dead {
        visuals.weak_text_color()
    } else {
        visuals.strong_text_color()
    };
    let left = inner.left() + 16.0;
    let font = FontId::proportional(14.0);
    let name = fitted(ui, &statics.name, font, color, right - left);
    let at = pos2(left, line - name.size().y / 2.0);
    painter.galley(at, name, color);
    // The second: energy.
    let bar = Rect::from_min_max(
        pos2(inner.left(), inner.bottom() - 14.0),
        pos2(inner.right(), inner.bottom()),
    );
    energy_bar(&painter, bar, energy);
    response
}

/// A robot's name laid out in at most `width`: in full if it fits, else without its
/// package, truncated if that doesn't fit either.
fn fitted(
    ui: &egui::Ui,
    name: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let full = ui
        .painter()
        .layout_no_wrap(name.to_string(), font.clone(), color);
    if full.size().x <= width {
        return full;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(short_name(name), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    ui.painter().layout_job(job)
}

/// A robot's name without its package: `jk.mega.DrussGT 3.1.16` is `DrussGT 3.1.16`.
fn short_name(name: &str) -> String {
    let (class, rest) = name.split_once(' ').unwrap_or((name, ""));
    let short = class.rsplit('.').next().unwrap_or(class);
    if rest.is_empty() {
        short.to_string()
    } else {
        format!("{short} {rest}")
    }
}

/// A small square in the robot's body colour.
fn swatch(
    painter: &egui::Painter,
    center: egui::Pos2,
    statics: &v1::RobotStatics,
    robot: Option<&v1::RobotSnapshot>,
) {
    let [r, g, b] = match robot {
        Some(robot) => view::colors(statics, robot).0[0],
        None => crate::sprites::default_colors(&statics.full_class_name)[0],
    };
    let square = Rect::from_center_size(center, vec2(10.0, 10.0));
    painter.rect_filled(square, 0.0, Color32::from_rgb(r, g, b));
    painter.rect_stroke(
        square,
        0.0,
        Stroke::new(1.0, Color32::from_white_alpha(60)),
        egui::StrokeKind::Inside,
    );
}

/// An energy bar: square, filled for the energy up to 100 (empty at 0), the value on it.
pub fn energy_bar(painter: &egui::Painter, rect: Rect, energy: f64) {
    painter.rect_filled(rect, 0.0, shell::BAR);
    let fraction = (energy / 100.0).clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
        let color = if energy >= 30.0 {
            ENERGY
        } else if energy >= 10.0 {
            ENERGY_LOW
        } else {
            ENERGY_CRITICAL
        };
        painter.rect_filled(fill, 0.0, color);
    }
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, shell::LINE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{energy:.1}"),
        FontId::proportional(11.0),
        Color32::from_gray(235),
    );
}

/// The robots' output: a tab per robot, with its debug-paint marker in the tab, over the
/// selected robot's output (`text`). Returns the robot whose marker was clicked.
pub fn output_panel<'a>(
    root: &mut egui::Ui,
    id: &str,
    robots: &[v1::RobotStatics],
    snapshot: Option<&v1::TurnSnapshot>,
    selected: &mut usize,
    paint: &[bool],
    text: impl FnOnce(usize) -> Cow<'a, str>,
) -> Option<usize> {
    let mut toggled = None;
    // A low window keeps most of its height for the field.
    let height = (root.available_height() * 0.22).clamp(90.0, 200.0);
    egui::Panel::bottom(id.to_string())
        .resizable(true)
        .default_size(height)
        .size_range(64.0..=480.0)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(shell::BAR))
        .show(root, |ui| {
            egui::Frame::NONE
                .fill(shell::PANEL)
                .inner_margin(egui::Margin {
                    left: 6,
                    right: 6,
                    top: 4,
                    bottom: 0,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::horizontal()
                        .id_salt("output-tabs")
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                // Names share the width (truncated), down to a minimum.
                                let share = ui.available_width() / robots.len().max(1) as f32;
                                let names = (share - TAB_EXTRA - 2.0).clamp(56.0, 180.0);
                                for (i, statics) in robots.iter().enumerate() {
                                    let robot = snapshot.and_then(|s| s.robots.get(i));
                                    let on = paint.get(i).copied().unwrap_or(false);
                                    let open = *selected == i;
                                    let tab = output_tab(ui, statics, robot, open, on, names);
                                    if tab.toggle {
                                        toggled = Some(i);
                                    } else if tab.clicked {
                                        *selected = i;
                                    }
                                }
                            });
                        });
                });
            let i = (*selected).min(robots.len().saturating_sub(1));
            let text = text(i);
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("output", i))
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if text.is_empty() {
                                ui.label(RichText::new("No output yet.").weak().italics());
                            } else {
                                ui.add(
                                    egui::Label::new(RichText::new(text.as_ref()).monospace())
                                        .wrap(),
                                );
                            }
                        });
                });
        });
    toggled
}

/// What happened to an output tab: clicked (open it), or its paint marker clicked.
struct Tab {
    clicked: bool,
    toggle: bool,
}

/// The paint marker's size in an output tab, and a tab's width besides the name.
const MARKER: f32 = 22.0;
const TAB_EXTRA: f32 = 6.0 + MARKER + 4.0 + 10.0 + 6.0 + 12.0;

/// An output tab: the paint marker, the robot's colour and its name (at most `names`
/// wide); the open one joins the output below it.
fn output_tab(
    ui: &mut egui::Ui,
    statics: &v1::RobotStatics,
    robot: Option<&v1::RobotSnapshot>,
    open: bool,
    paint: bool,
    names: f32,
) -> Tab {
    const HEIGHT: f32 = 28.0;
    let visuals = ui.visuals().clone();
    let color = if open {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    let name = fitted(ui, &statics.name, FontId::proportional(13.0), color, names);
    let width = TAB_EXTRA + name.size().x;
    let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
    let marker = Rect::from_center_size(
        pos2(rect.left() + 6.0 + MARKER / 2.0, rect.center().y),
        vec2(MARKER, MARKER),
    );
    let on_marker = response
        .interact_pointer_pos()
        .or(response.hover_pos())
        .is_some_and(|p| marker.contains(p));
    let response = if on_marker {
        let hint = if paint {
            "Debug paint on: click to turn it off"
        } else {
            "Debug paint off: click to turn it on"
        };
        response.on_hover_text(hint)
    } else {
        response.on_hover_text(format!("{}: show its output", statics.name))
    };
    let painter = ui.painter_at(rect);
    let fill = if open {
        Some(shell::BAR)
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    if let Some(fill) = fill {
        painter.rect_filled(
            rect,
            egui::CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            },
            fill,
        );
    }
    if open {
        let line = Rect::from_min_size(rect.min, vec2(rect.width(), 2.0));
        painter.rect_filled(line, 0.0, shell::ACCENT);
    }
    if on_marker {
        painter.rect_filled(marker, 3.0, visuals.widgets.active.weak_bg_fill);
    }
    let marker_color = if paint {
        shell::ACCENT
    } else if on_marker {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color().gamma_multiply(0.45)
    };
    icons::pencil(&painter, marker.center(), 6.0, marker_color);
    swatch(
        &painter,
        pos2(marker.right() + 4.0 + 5.0, rect.center().y),
        statics,
        robot,
    );
    let at = pos2(
        marker.right() + 4.0 + 10.0 + 6.0,
        rect.center().y - name.size().y / 2.0,
    );
    painter.galley(at, name, color);
    let clicked = response.clicked();
    Tab {
        clicked,
        toggle: clicked && on_marker,
    }
}

/// The controls below the field: `content` lays them out from the left.
pub fn controls_panel(root: &mut egui::Ui, id: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Panel::bottom(id.to_string())
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::NONE
                .fill(shell::PANEL)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .stroke(Stroke::new(1.0, shell::LINE)),
        )
        .show(root, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            content(ui);
        });
}

/// A control: an icon button, disabled unless `enabled`.
pub fn control(
    ui: &mut egui::Ui,
    enabled: bool,
    hint: &str,
    draw: impl FnOnce(&egui::Painter, egui::Pos2, Color32),
) -> bool {
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, CONTROL, hint, true, draw))
        .inner
        .clicked()
}

/// A thin vertical line between groups of controls.
pub fn divider(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, CONTROL - 8.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, shell::LINE);
    ui.add_space(6.0);
}

/// The speed slider's widest, and its narrowest with the seed still shown and at all.
const SLIDER: f32 = 130.0;
const SLIDER_NARROW: f32 = 80.0;
const SLIDER_MIN: f32 = 40.0;
/// The speed slider's icon and its value's label.
const SPEED_ICON: f32 = 22.0;
const SPEED_LABEL: f32 = 44.0;

/// The controls row after its buttons: the speed slider, then the readouts at the right
/// end, added right to left: `end` (rightmost), the seed, `rest`, and `fill`, which may
/// take the room that is left (a truncated label). The slider gets the room the readouts
/// left last frame, up to [`SLIDER`]; when that leaves it narrower than [`SLIDER_NARROW`],
/// the seed goes (a viewer track has none). True when the speed changed.
pub fn speed_and_readouts(
    ui: &mut egui::Ui,
    turns_per_second: &mut u32,
    seed: Option<&str>,
    end: impl FnOnce(&mut egui::Ui),
    rest: impl FnOnce(&mut egui::Ui),
    fill: impl FnOnce(&mut egui::Ui),
) -> bool {
    let id = ui.id().with("readouts");
    // The readouts' width without the seed, and the seed's with its gap, last measured.
    let (others, seed_width) = ui
        .data(|d| d.get_temp::<(f32, f32)>(id))
        .unwrap_or((160.0, 170.0));
    let spacing = ui.spacing().item_spacing.x;
    let room = ui.available_width() - SPEED_ICON - SPEED_LABEL - 3.0 * spacing - others;
    let seed_width = if seed.is_some() { seed_width } else { 0.0 };
    let show_seed = seed.is_some() && room - seed_width >= SLIDER_NARROW;
    let width = if show_seed { room - seed_width } else { room };
    let changed = speed_slider(ui, turns_per_second, width.clamp(SLIDER_MIN, SLIDER));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let end = ui.scope(end).response.rect.width();
        let mut seed_width = seed_width;
        if let Some(seed) = seed.filter(|_| show_seed) {
            let shown = ui.scope(|ui| info(ui, "Seed", seed)).response.rect.width();
            ui.add_space(10.0);
            seed_width = shown + 10.0 + 2.0 * spacing;
        }
        let rest = ui.scope(rest).response.rect.width();
        fill(ui);
        let others = end + rest + 2.0 * spacing;
        ui.data_mut(|d| d.insert_temp(id, (others, seed_width)));
    });
    changed
}

/// The speed: a turn rate from 1 to [`MAX_TPS`] a second, or as fast as possible (0) at
/// the slider's right end, the slider `width` wide. True when it changed.
fn speed_slider(ui: &mut egui::Ui, turns_per_second: &mut u32, width: f32) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(SPEED_ICON, CONTROL), Sense::hover());
    icons::speed(
        ui.painter(),
        rect.center(),
        7.0,
        ui.visuals().weak_text_color(),
    );
    let mut position = match *turns_per_second {
        0 => MAX_TPS + 1,
        tps => tps.min(MAX_TPS),
    };
    ui.spacing_mut().slider_width = width;
    let slider = ui
        .add(
            egui::Slider::new(&mut position, 1..=MAX_TPS + 1)
                .logarithmic(true)
                .show_value(false),
        )
        .on_hover_text("Turns a second");
    let text = match position {
        p if p > MAX_TPS => "max".to_string(),
        p => format!("{p}/s"),
    };
    let (rect, _) = ui.allocate_exact_size(vec2(SPEED_LABEL, CONTROL), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(13.0),
        ui.visuals().text_color(),
    );
    if slider.changed() {
        *turns_per_second = if position > MAX_TPS { 0 } else { position };
    }
    slider.changed()
}

/// A labelled value, such as the turn, in a right-to-left layout (the controls' right
/// end): the label, then the value.
pub fn info(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.label(RichText::new(value.into()).strong());
    ui.label(RichText::new(label).weak());
}

/// The results at the end of a battle, live or replayed: what the results window shows.
pub struct Outcome<'a> {
    /// The window's title.
    pub title: &'a str,
    pub robots: &'a [v1::RobotStatics],
    pub results: &'a [v1::Results],
    /// The last turn, for the robots' own colours.
    pub snapshot: Option<&'a v1::TurnSnapshot>,
    /// Stopped before its end: the first is leading rather than the winner.
    pub stopped: bool,
}

/// The results window, centred when it opens (once per `id`): the winner on the left (its
/// tank in its colours, its name), classic's results table on the right, and `buttons`
/// below, right-aligned (add them right to left).
pub fn results_window(
    ctx: &egui::Context,
    sprites: &mut Sprites,
    id: egui::Id,
    outcome: &Outcome,
    buttons: impl FnOnce(&mut egui::Ui),
) {
    let rows = contestants(outcome.robots, outcome.results);
    let screen = ctx.content_rect();
    egui::Window::new(outcome.title)
        .id(id)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(screen.center())
        .max_width(screen.width() - 40.0)
        .show(ctx, |ui| {
            let row = ui.horizontal_top(|ui| {
                let left = winner(ui, sprites, outcome, &rows);
                ui.add_space(GAP);
                // The table as wide as it was last time; narrow windows scroll it.
                let measured = id.with("table-width");
                let wanted = ui.data(|d| d.get_temp::<f32>(measured)).unwrap_or(600.0);
                let room = ui.ctx().content_rect().width() - 80.0 - WINNER - GAP;
                let shown = egui::ScrollArea::horizontal()
                    .min_scrolled_width(wanted.min(room).max(120.0))
                    .show(ui, |ui| table(ui, &rows));
                ui.data_mut(|d| d.insert_temp(measured, shown.content_size.x));
                left.right() + GAP / 2.0
            });
            // The line between them, as tall as the row (the window's contents).
            let rect = row.response.rect;
            ui.painter()
                .vline(row.inner, rect.y_range(), Stroke::new(1.0, shell::LINE));
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(2.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 28.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().button_padding = vec2(10.0, 4.0);
                    buttons(ui);
                },
            );
        });
}

/// The width of the winner's column in the results window.
const WINNER: f32 = 150.0;
/// The space between the winner and the table, with a line in its middle.
const GAP: f32 = 24.0;

/// The results window's left side: the winner, the contestants tied for first, the one
/// leading a stopped battle, or nobody when no one scored.
fn winner(
    ui: &mut egui::Ui,
    sprites: &mut Sprites,
    outcome: &Outcome,
    rows: &[Contestant],
) -> Rect {
    let best = rows.first().map_or(0, |c| round(c.score));
    let first: Vec<&Contestant> = rows
        .iter()
        .take_while(|c| best > 0 && round(c.score) == best)
        .collect();
    let heading = match (first.len(), outcome.stopped) {
        (0, _) => "No winner",
        (1, false) => "Winner",
        (1, true) => "Leading",
        (_, false) => "Tied for first",
        (_, true) => "Tied in the lead",
    };
    ui.vertical(|ui| {
        ui.set_width(WINNER);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(heading.to_uppercase())
                    .size(13.0)
                    .strong()
                    .color(shell::ACCENT),
            );
            if first.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Nobody scored.").weak());
            }
            for c in first.iter().take(3) {
                tank(ui, sprites, outcome, &c.name);
                ui.add(egui::Label::new(RichText::new(&c.name).strong().size(15.0)).truncate());
                ui.label(RichText::new(format!("{} points", round(c.score))).weak());
                ui.add_space(4.0);
            }
            if first.len() > 3 {
                ui.label(RichText::new(format!("and {} more", first.len() - 3)).weak());
            }
        });
    })
    .response
    .rect
}

/// A contestant's tank in its colours as of the last turn (a team: its first member's).
fn tank(ui: &mut egui::Ui, sprites: &mut Sprites, outcome: &Outcome, name: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
    let index = outcome
        .robots
        .iter()
        .position(|r| r.team_name.as_deref().unwrap_or(&r.name) == name);
    let colors = match index {
        Some(i) => {
            let statics = &outcome.robots[i];
            match outcome.snapshot.and_then(|s| s.robots.get(i)) {
                Some(snapshot) => view::colors(statics, snapshot).0,
                None => default_colors(&statics.full_class_name),
            }
        }
        None => default_colors(name),
    };
    let ctx = ui.ctx().clone();
    for (part, color) in [Part::Body, Part::Gun, Part::Radar].into_iter().zip(colors) {
        let (texture, size) = sprites.tank(&ctx, part, color);
        let shape = view::sprite(texture, rect.center(), size * 1.7, 0.0, Color32::WHITE);
        ui.painter().add(shape);
    }
}

/// Classic's results table: names left-aligned, numbers right-aligned.
fn table(ui: &mut egui::Ui, rows: &[Contestant]) {
    egui::Grid::new("results-table")
        .striped(true)
        .spacing(vec2(14.0, 6.0))
        .show(ui, |ui| {
            for (col, h) in COLUMNS.iter().enumerate() {
                cell(ui, col, RichText::new(*h).strong());
            }
            ui.end_row();
            for row in cells(rows) {
                for (col, text) in row.into_iter().enumerate() {
                    cell(ui, col, RichText::new(text));
                }
                ui.end_row();
            }
        });
}

/// A cell of the results table: the robot's name left-aligned, the rest right-aligned.
fn cell(ui: &mut egui::Ui, col: usize, text: RichText) {
    if col == 1 {
        ui.label(text);
    } else {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(text)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::short_name;

    #[test]
    fn short_names_drop_the_package() {
        assert_eq!(short_name("sample.SittingDuck"), "SittingDuck");
        assert_eq!(short_name("jk.mega.DrussGT 3.1.16"), "DrussGT 3.1.16");
        assert_eq!(short_name("sample.Fire (2)"), "Fire (2)");
        assert_eq!(short_name("Plain"), "Plain");
    }
}
