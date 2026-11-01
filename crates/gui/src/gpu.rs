// SPDX-License-Identifier: EPL-2.0

//! Losing the GPU. Windows takes the GPU device away now and then (the display going to
//! sleep or waking, a driver reset), and egui-wgpu 0.36 panics on the next frame
//! ("Failed to create staging buffer ...") with no way to recover in place: the textures
//! it uploaded can't be sent again. So `main` catches that panic and opens a new window
//! on a new device, while the page's threads (contributing to the rumble) go on. Only on
//! Windows: winit 0.30 on X11 and Wayland can't run its event loop again after a panic
//! (its `loop_running` stays set), so there the panic ends the app as before, logged.
//!
//! This module tells `main` whether a panic was such a loss, and logs (in
//! `logs/robocode-s.log`) what might lead to a better fix: the device-lost callback's
//! reason and message, wgpu's uncaptured errors, the adapter and driver, and what the
//! window did shortly before (minimised, focus, monitor and scale changes, the last
//! frame).
//!
//! `ROBOCODES_LOSE_GPU=SECONDS` destroys the first window's device that many seconds
//! after it opens, to try the recovery.

use eframe::egui;
use eframe::wgpu;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const LOSE_VAR: &str = "ROBOCODES_LOSE_GPU";
/// How many of the window's recent changes the log gets at a loss.
const HISTORY: usize = 24;
/// A frame further than this from the previous one is noted in the history.
const GAP: Duration = Duration::from_secs(30);

/// What the window and the device did, for the log at a loss.
struct Watch {
    /// The windows opened so far (1 is the first).
    window: u32,
    opened: Option<Instant>,
    /// The device, kept to poll it after a panic: that runs a device-lost callback that
    /// was still waiting.
    device: Option<wgpu::Device>,
    adapter: String,
    frames: u64,
    last_frame: Option<Instant>,
    view: View,
    history: VecDeque<(Instant, String)>,
    /// The device-lost callback ran for this window: (when, reason, message).
    lost: Option<(Instant, wgpu::DeviceLostReason, String)>,
    /// wgpu's uncaptured errors in this window: (when, is "out of memory", text).
    errors: Vec<(Instant, bool, String)>,
}

/// The window state the history notes the changes of.
#[derive(Clone, Default, PartialEq)]
struct View {
    minimized: Option<bool>,
    focused: Option<bool>,
    monitor: Option<(i32, i32)>,
    scale: Option<i32>,
    size: Option<(i32, i32)>,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch {
    window: 0,
    opened: None,
    device: None,
    adapter: String::new(),
    frames: 0,
    last_frame: None,
    view: View {
        minimized: None,
        focused: None,
        monitor: None,
        scale: None,
        size: None,
    },
    history: VecDeque::new(),
    lost: None,
    errors: Vec::new(),
});

/// `ROBOCODES_LOSE_GPU` acts once in the process, so the new window keeps its device.
static LOSE_DONE: AtomicBool = AtomicBool::new(false);

fn watch() -> MutexGuard<'static, Watch> {
    // A panic while holding it leaves the data good enough for a log.
    WATCH.lock().unwrap_or_else(|e| e.into_inner())
}

fn log(text: &str) {
    rumble_client::log::line(&format!("gpu: {text}"));
}

fn ago(at: Instant, now: Instant) -> String {
    format!("{:.1} s", now.saturating_duration_since(at).as_secs_f64())
}

pub fn describe(info: &wgpu::AdapterInfo) -> String {
    format!(
        "{} ({:?}, {:?}, driver {} {}, vendor {:#06x} device {:#06x})",
        info.name,
        info.backend,
        info.device_type,
        info.driver,
        info.driver_info,
        info.vendor,
        info.device
    )
}

/// A new window: logs its adapter and watches its device. Call it from the app's creator.
pub fn install(cc: &eframe::CreationContext) {
    let Some(state) = &cc.wgpu_render_state else {
        return;
    };
    let adapter = describe(&state.adapter.get_info());
    let window = {
        let mut w = watch();
        w.window += 1;
        w.opened = Some(Instant::now());
        w.device = Some(state.device.clone());
        w.adapter = adapter.clone();
        w.frames = 0;
        w.last_frame = None;
        w.view = View::default();
        w.lost = None;
        w.errors.clear();
        let window = w.window;
        w.history
            .push_back((Instant::now(), format!("window {window} opened")));
        while w.history.len() > HISTORY {
            w.history.pop_front();
        }
        window
    };
    log(&format!(
        "window {window} on {adapter}; surface {:?}, present mode {:?}, frame latency {:?}; {} \
         adapters",
        state.target_format,
        state.surface_config.present_mode,
        state.surface_config.desired_maximum_frame_latency,
        state.available_adapters.len()
    ));
    state
        .device
        .set_device_lost_callback(move |reason, message| {
            let now = Instant::now();
            let report = {
                let mut w = watch();
                if w.window != window {
                    // An old window's device going away (dropped after a restart).
                    return;
                }
                w.lost = Some((now, reason, message.clone()));
                report(&w, now)
            };
            log(&format!(
                "window {window} lost its device ({reason:?}): {message}\n{report}"
            ));
        });
    state
        .device
        .on_uncaptured_error(std::sync::Arc::new(move |error: wgpu::Error| {
            let now = Instant::now();
            let oom = matches!(error, wgpu::Error::OutOfMemory { .. });
            let text = error.to_string();
            {
                let mut w = watch();
                if w.window == window && w.errors.len() < 20 {
                    w.errors.push((now, oom, text.clone()));
                }
            }
            log(&format!("window {window}, uncaptured wgpu error: {text}"));
            // As wgpu's default handler does.
            panic!("wgpu error: {error}\n");
        }));
}

/// What the log gets at a loss: the window's age, frames, adapter and recent history.
fn report(w: &Watch, now: Instant) -> String {
    let mut text = String::new();
    let _ = write!(
        text,
        "  adapter {}\n  window open {}, {} frames",
        w.adapter,
        w.opened.map_or_else(|| "?".into(), |o| ago(o, now)),
        w.frames
    );
    if let Some(last) = w.last_frame {
        let _ = write!(text, ", the last {} ago", ago(last, now));
    }
    let v = &w.view;
    let _ = write!(
        text,
        "\n  minimised {:?}, focused {:?}, monitor {:?}, scale {:?}%, size {:?}",
        v.minimized, v.focused, v.monitor, v.scale, v.size
    );
    for (at, error) in w.errors.iter().map(|(at, _, e)| (at, e)) {
        let _ = write!(text, "\n  uncaptured error {} ago: {error}", ago(*at, now));
    }
    text.push_str("\n  recently:");
    for (at, what) in &w.history {
        let _ = write!(text, "\n    {} ago: {what}", ago(*at, now));
    }
    text
}

/// Call at the start of each frame: counts it and notes the window's changes.
pub fn frame(ctx: &egui::Context) {
    let now = Instant::now();
    let view = ctx.input(|i| {
        let v = i.viewport();
        View {
            minimized: v.minimized,
            focused: v.focused,
            monitor: v.monitor_size.map(|s| (s.x as i32, s.y as i32)),
            scale: v
                .native_pixels_per_point
                .map(|p| (p * 100.0).round() as i32),
            size: v.inner_rect.map(|r| (r.width() as i32, r.height() as i32)),
        }
    });
    let lose = {
        let mut w = watch();
        w.frames += 1;
        let mut notes = Vec::new();
        if let Some(last) = w.last_frame
            && now.duration_since(last) >= GAP
        {
            notes.push(format!("a frame after {} without", ago(last, now)));
        }
        let old = std::mem::replace(&mut w.view, view.clone());
        if w.frames > 1 && old != view {
            notes.push(changes(&old, &view));
        }
        w.last_frame = Some(now);
        for note in notes {
            w.history.push_back((now, note));
        }
        while w.history.len() > HISTORY {
            w.history.pop_front();
        }
        lose_now(&w, now)
    };
    if let Some(device) = lose {
        log(&format!("{LOSE_VAR}: destroying the device"));
        device.destroy();
    }
}

/// The device to destroy now for `ROBOCODES_LOSE_GPU`, once.
fn lose_now(w: &Watch, now: Instant) -> Option<wgpu::Device> {
    if LOSE_DONE.load(Ordering::Relaxed) {
        return None;
    }
    let secs: f64 = std::env::var(LOSE_VAR).ok()?.trim().parse().ok()?;
    let opened = w.opened?;
    if now.duration_since(opened).as_secs_f64() < secs {
        return None;
    }
    LOSE_DONE.store(true, Ordering::Relaxed);
    w.device.clone()
}

fn changes(old: &View, new: &View) -> String {
    let mut parts = Vec::new();
    if old.minimized != new.minimized {
        parts.push(format!("minimised {:?}", new.minimized));
    }
    if old.focused != new.focused {
        parts.push(format!("focused {:?}", new.focused));
    }
    if old.monitor != new.monitor {
        parts.push(format!("monitor {:?} -> {:?}", old.monitor, new.monitor));
    }
    if old.scale != new.scale {
        parts.push(format!("scale {:?}% -> {:?}%", old.scale, new.scale));
    }
    if old.size != new.size {
        parts.push(format!("size {:?} -> {:?}", old.size, new.size));
    }
    parts.join(", ")
}

/// Why `main` thinks a window's panic was the GPU going away (none: it wasn't).
#[derive(Debug, PartialEq)]
pub struct Loss {
    pub signs: Vec<&'static str>,
}

/// After the window's event loop panicked with `message`: was it the GPU going away?
/// Polls the old device (which runs a device-lost callback that was still waiting), logs
/// what was seen, and lets the device go.
pub fn after_panic(message: &str) -> Option<Loss> {
    let device = watch().device.take();
    let polled = device.as_ref().map(|d| d.poll(wgpu::PollType::Poll));
    let now = Instant::now();
    let w = watch();
    let signs = signs(message, w.lost.is_some(), w.errors.iter().any(|e| e.1));
    let mut text = format!(
        "window {} panicked: {}",
        w.window,
        message.lines().next().unwrap_or("")
    );
    let _ = write!(
        text,
        "\n  device-lost callback: {}",
        match &w.lost {
            Some((at, reason, m)) => format!("{reason:?} {} ago: {m}", ago(*at, now)),
            None => "didn't run".into(),
        }
    );
    if let Some(polled) = polled {
        let _ = write!(text, "\n  polling the device after: {polled:?}");
    }
    let _ = write!(text, "\n{}", report(&w, now));
    let _ = write!(
        text,
        "\n  verdict: {}",
        if signs.is_empty() {
            "not the GPU going away".to_string()
        } else {
            format!("the GPU went away ({})", signs.join(", "))
        }
    );
    drop(w);
    log(&text);
    drop(device);
    (!signs.is_empty()).then_some(Loss { signs })
}

/// The signs that a panic was the GPU going away.
fn signs(message: &str, lost: bool, out_of_memory: bool) -> Vec<&'static str> {
    let mut signs = Vec::new();
    if lost {
        signs.push("the device-lost callback ran");
    }
    // egui-wgpu's renderer, when the queue gives no staging buffer (a lost device).
    if message.starts_with("Failed to create staging buffer") {
        signs.push("egui-wgpu got no staging buffer");
    }
    if out_of_memory {
        signs.push("wgpu ran out of memory");
    }
    signs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_of_a_loss() {
        let staging = "Failed to create staging buffer for index data. Index count: 8847.";
        assert_eq!(
            signs(staging, false, false),
            ["egui-wgpu got no staging buffer"]
        );
        assert_eq!(
            signs("wgpu error: Validation Error", true, false),
            ["the device-lost callback ran"]
        );
        assert_eq!(
            signs("wgpu error: Out of Memory", false, true),
            ["wgpu ran out of memory"]
        );
        assert!(signs("index out of bounds: the len is 3", false, false).is_empty());
    }

    #[test]
    fn changes_name_what_changed() {
        let old = View {
            minimized: Some(false),
            focused: Some(true),
            monitor: Some((2560, 1440)),
            scale: Some(100),
            size: Some((1200, 860)),
        };
        let new = View {
            focused: Some(false),
            monitor: None,
            ..old.clone()
        };
        assert_eq!(
            changes(&old, &new),
            "focused Some(false), monitor Some((2560, 1440)) -> None"
        );
    }
}
