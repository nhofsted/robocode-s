// SPDX-License-Identifier: EPL-2.0

//! Our own window frame (title bar, buttons, resize edges), for a window like Steam's on
//! Windows and Linux (and because the system's crashes under WSLg); and the window's
//! minimum size (see [`keep_min_size`]).
//!
//! On Linux the moves and resizes go to the window manager or compositor
//! (`_NET_WM_MOVERESIZE` on X11, `xdg_toplevel.move`/`resize` on Wayland), which then
//! holds the pointer (see [`hand_over`]).
//!
//! On WSLg the compositor (Weston 9 with its pixman renderer) draws no decorations, so
//! winit draws its own frame from a `wl_shm` pool that it grows as the window gets wider.
//! Weston segfaults in pixman when that pool is resized while it holds the old buffer,
//! which takes the app down with it: dragging the window wider crashed it (#28). Without
//! winit's frame there is no pool to resize; egui draws the frame instead.
//!
//! On Windows winit's undecorated window keeps its caption and sizing styles and only
//! hides the non-client area (`WM_NCCALCSIZE`), so the system still snaps it (Win+arrows,
//! dragging to a screen edge) and gives it a drop shadow (egui-winit asks for one when
//! the window has no decorations); maximized, it covers the work area, not the taskbar.
//! Moves and resizes run the system's own loops (`WM_NCLBUTTONDOWN`), which hold the
//! window at its minimum size.
//!
//! winit gets that shadow by keeping the window's top pixel row out of the client area,
//! and Windows 11 draws its 1px window border around the window, over that row too: a
//! grey line above the top bar. [`hide_system_border`] turns the border off and colours
//! the caption (without that the row stays a dark grey); the row then shows what is
//! behind the window, like the shadow around it, so the frame is 0px, and the shadow and
//! the rounded corners stay. Windows 10 draws no such border, and has neither colour.

use eframe::egui::{self, Color32, CursorIcon, Id, PointerButton, Sense, Stroke, vec2};
use egui::viewport::{ResizeDirection, ViewportCommand};

/// Width of the resize edges, in points.
const EDGE: f32 = 5.0;
/// Length of the corner zones along each edge.
const CORNER: f32 = 16.0;
const BUTTON_WIDTH: f32 = 44.0;
/// Height of the top bar: the title bar with our frame, the app's top bar without it.
pub const BAR_HEIGHT: f32 = 40.0;

/// Whether to draw our own frame: everywhere but on macOS (whose title bar keeps its
/// traffic lights), unless `ROBOCODES_OWN_FRAME` says otherwise (`1` or `0`).
pub fn wanted() -> bool {
    match std::env::var("ROBOCODES_OWN_FRAME").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => !cfg!(target_os = "macos"),
    }
}

/// Hides the system's border of our own frame on Windows 11 (see the module's docs):
/// none around the window, and the caption in `top`, the title bar's colour.
#[cfg(windows)]
pub fn hide_system_border(cc: &eframe::CreationContext, top: Color32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_COLOR_NONE,
    };
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    // A COLORREF: 0x00BBGGRR.
    let caption = u32::from(top.r()) | (u32::from(top.g()) << 8) | (u32::from(top.b()) << 16);
    dwm::set_color(hwnd, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
    dwm::set_color(hwnd, DWMWA_CAPTION_COLOR, caption);
}

#[cfg(not(windows))]
pub fn hide_system_border(_cc: &eframe::CreationContext, _top: Color32) {}

#[cfg(windows)]
mod dwm {
    #![allow(unsafe_code)] // one Win32 call, with a window handle and a plain value
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};

    /// Sets a colour attribute; before Windows 11 that fails, harmlessly.
    pub fn set_color(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, color: u32) {
        // SAFETY: `hwnd` is our live window; the value is a u32 of the size given.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&raw const color).cast(),
                size_of::<u32>() as u32,
            )
        };
    }
}

/// Whether the window system tells us when a move or resize we handed it is over. On
/// Windows winit posts the button's release when the system's move loop ends
/// (`WM_EXITSIZEMOVE`); a made-up one ([`release_after_grab`]) would end egui's drag
/// while the loop still runs.
const RELEASES_AFTER_GRAB: bool = cfg!(windows);

/// Draws the frame around the window's content; call it before the other panels. The
/// title bar holds `content` (the app's top bar) left of the window buttons; its empty
/// parts drag the window, and a double click maximizes it.
pub fn show(root: &mut egui::Ui, fill: Color32, content: impl FnOnce(&mut egui::Ui)) {
    let ctx = root.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    if !maximized {
        edges(root, fill);
    }
    egui::Panel::top("window-title")
        .exact_size(BAR_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(fill))
        .show(root, |ui| {
            let rect = ui.max_rect();
            // First, so that the widgets on top of it take their own clicks.
            let bar = ui.interact(rect, Id::new("window-title-bar"), Sense::click_and_drag());
            if bar.drag_started_by(PointerButton::Primary) {
                hand_over(&ctx, ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
            let buttons = rect.with_min_x(rect.right() - 3.0 * BUTTON_WIDTH);
            let rest = rect.with_max_x(buttons.left() - 8.0);
            ui.scope_builder(egui::UiBuilder::new().max_rect(rest), content);
            ui.scope_builder(egui::UiBuilder::new().max_rect(buttons), |ui| {
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

/// Where [`hand_over`] notes that the compositor has the pointer.
fn grab_id() -> Id {
    Id::new("window-grab")
}

/// Hands a move or resize to the compositor. From then on the compositor holds the
/// pointer, so the button's release never reaches us (winit only reports the pointer
/// leaving, which egui doesn't take as a release): egui would think the button still
/// down, keep the edge or title bar dragged, with its cursor, until Escape.
/// [`release_after_grab`] makes up the release (not on Windows, see
/// [`RELEASES_AFTER_GRAB`]).
fn hand_over(ctx: &egui::Context, command: ViewportCommand) {
    log(&format!("{command:?}: the compositor takes the pointer"));
    if RELEASES_AFTER_GRAB {
        ctx.send_viewport_cmd(command);
        return;
    }
    ctx.data_mut(|d| {
        d.insert_temp(grab_id(), true);
        if matches!(command, ViewportCommand::BeginResize(_)) {
            d.insert_temp(resize_id(), Resize { ended: None });
        }
    });
    ctx.send_viewport_cmd(command);
}

/// Adds the primary button's release to the input of the frame after a move or resize
/// was handed to the compositor (see [`hand_over`]); call it from
/// [`eframe::App::raw_input_hook`].
pub fn release_after_grab(ctx: &egui::Context, raw_input: &mut egui::RawInput) {
    for event in &raw_input.events {
        if matches!(
            event,
            egui::Event::PointerButton { .. } | egui::Event::PointerGone
        ) {
            log(&format!("input: {event:?}"));
        }
    }
    let grabbed = ctx.data_mut(|d| d.remove_temp::<bool>(grab_id()));
    if grabbed != Some(true) {
        // The pointer is back: the compositor let go of it, so a resize is over. (The
        // frame that makes up the release is skipped: what it brings may be from before
        // the compositor took the pointer.)
        let back = raw_input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerButton { .. }
                    | egui::Event::PointerGone
            )
        });
        let resize = ctx.data_mut(|d| d.get_temp::<Resize>(resize_id()));
        if back && let Some(Resize { ended: None }) = resize {
            log("the pointer is back: the resize is over");
            let now = raw_input.time.unwrap_or_else(|| ctx.input(|i| i.time));
            ctx.data_mut(|d| d.insert_temp(resize_id(), Resize { ended: Some(now) }));
        }
        return;
    }
    let pos = ctx
        .input(|i| i.pointer.latest_pos().or(i.pointer.interact_pos()))
        .unwrap_or_default();
    log("made up the primary button's release");
    raw_input.events.push(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: ctx.input(|i| i.modifiers),
    });
}

/// How long the window's size has to stay put before [`keep_min_size`] grows it: long
/// enough not to fight a resize in progress.
const SETTLE: f64 = 0.4;

/// Where [`hand_over`] notes a resize it gave the compositor.
fn resize_id() -> Id {
    Id::new("window-resize")
}

/// A resize the compositor does for us (see [`hand_over`]).
#[derive(Clone, Copy, Default)]
struct Resize {
    /// When (the UI's time) the pointer came back, which ends it.
    ended: Option<f64>,
}

/// How long after a resize [`keep_min_size`] still takes the window being too small as
/// the resize's doing, and grows it at once. The compositor's last word on a resize, a
/// configure without the resizing state, comes with the pointer's return.
const AFTER_RESIZE: f64 = 0.5;

/// What [`keep_min_size`] remembers between frames.
#[derive(Clone, Copy, Default)]
struct SizeWatch {
    size: egui::Vec2,
    /// Since when (the UI's time) the window has been `size`.
    since: f64,
    /// The too small size it last grew the window from, so as not to ask again and again
    /// when the window doesn't grow; cleared once the window has another size.
    asked: Option<egui::Vec2>,
    /// Whether it has asked winit to hold resizes at the minimum (see [`keep_min_size`]).
    hinted: bool,
}

/// Keeps the window at least `min` points big where the window system doesn't. The
/// minimum goes to the compositor too (`xdg_toplevel.set_min_size` on Wayland), but
/// sizes the app asks for aren't held to it, nor are the user's resizes by every
/// compositor: WSLg's Weston ignores the minimum in its resize grab and configures the
/// window down to a few pixels as the pointer goes, and winit takes the configured size
/// as it is. Call it every frame.
///
/// - During such a resize winit holds the window at the minimum itself, once it has
///   resize increments (asked for here, on Wayland; 1 point, so no steps): it measures
///   an interactive resize's size from the minimum, and doesn't go below it. So the
///   window never draws a frame below the minimum while the user drags.
/// - The configure that ends the resize (without the resizing state) still has the
///   small size the compositor made of the drag. Right after a resize we handed over
///   (see [`hand_over`]), a too small window grows back to the minimum at once.
/// - Otherwise, once a too small window has kept its size for a moment (so not in the
///   middle of a resize the window system does on its own), this asks for the minimum.
///
/// A maximized, fullscreen or minimized window is left alone.
pub fn keep_min_size(ctx: &egui::Context, min: egui::Vec2) {
    let id = Id::new("window-min-size");
    let (size, now, pinned) = ctx.input(|i| {
        let v = i.viewport();
        let pinned = [v.maximized, v.fullscreen, v.minimized].contains(&Some(true));
        (i.content_rect().size(), i.time, pinned)
    });
    let mut watch = ctx
        .data_mut(|d| d.get_temp::<SizeWatch>(id))
        .unwrap_or_default();
    if !watch.hinted {
        watch.hinted = true;
        // Elsewhere winit's increments could show as the window's size in steps.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            log("resize increments of 1 point, to hold resizes at the minimum");
            let one = egui::Vec2::splat(1.0);
            ctx.send_viewport_cmd(ViewportCommand::ResizeIncrements(Some(one)));
        }
    }
    let resize = ctx.data_mut(|d| d.get_temp::<Resize>(resize_id()));
    let resizing = match resize {
        Some(Resize { ended: Some(ended) }) if now - ended >= AFTER_RESIZE => {
            ctx.data_mut(|d| d.remove_temp::<Resize>(resize_id()));
            false
        }
        Some(_) => true,
        None => false,
    };
    if watch.size != size {
        log(&format!(
            "window {:.0}x{:.0} points (minimum {:.0}x{:.0}){}{}",
            size.x,
            size.y,
            min.x,
            min.y,
            if pinned {
                ", maximized, fullscreen or minimized"
            } else {
                ""
            },
            match resize {
                Some(Resize { ended: None }) => ", resizing",
                Some(_) if resizing => ", just resized",
                _ => "",
            }
        ));
        watch.size = size;
        watch.since = now;
        watch.asked = None;
    }
    // Half a point of slack for the rounding to whole pixels.
    let small = size.x < min.x - 0.5 || size.y < min.y - 0.5;
    if small && !pinned && watch.asked != Some(size) {
        let still = now - watch.since;
        if resizing || still >= SETTLE {
            let grown = size.max(min);
            log(&format!(
                "window below the minimum{}: asking for {:.0}x{:.0}",
                if resizing { " after a resize" } else { "" },
                grown.x,
                grown.y
            ));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(grown));
            watch.asked = Some(size);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE - still));
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, watch));
}

/// Logs the hand-overs, pointer buttons and window sizes to stderr when
/// `ROBOCODES_FRAME_LOG` is set.
fn log(message: &str) {
    if std::env::var_os("ROBOCODES_FRAME_LOG").is_some() {
        eprintln!("[frame] {message}");
    }
}

/// Thin panels along the four sides that start a resize when pressed.
fn edges(root: &mut egui::Ui, fill: Color32) {
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
                    hand_over(ui.ctx(), ViewportCommand::BeginResize(direction));
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
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_WIDTH, BAR_HEIGHT), Sense::click());
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
            // The front window, and of the one behind it only what shows.
            let front =
                egui::Rect::from_center_size(c + vec2(-1.5, 1.5), egui::Vec2::splat(1.6 * r));
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
            let (l, t, rt, b) = (
                front.left() + 3.5,
                front.top() - 3.0,
                front.right() + 3.0,
                front.bottom() - 3.5,
            );
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(l, front.top() + 0.5),
                    egui::pos2(l, t + 0.5),
                    egui::pos2(rt - 0.5, t + 0.5),
                    egui::pos2(rt - 0.5, b),
                    egui::pos2(front.right(), b),
                ],
                stroke,
            ));
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
