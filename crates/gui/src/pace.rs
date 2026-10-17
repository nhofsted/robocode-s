// SPDX-License-Identifier: EPL-2.0

//! How fast the animations draw (the backdrop, the battles, replays): at the display's
//! refresh rate, or at most the frame rate set in the options. Nothing asks for frames
//! while nothing moves.

use eframe::egui;
use std::time::{Duration, Instant};

/// The frame rates the options offer; 0 is the display's.
pub const RATES: [u32; 4] = [0, 30, 60, 120];

/// What [`begin`] notes for the frame.
#[derive(Clone, Copy)]
struct Pace {
    /// The most frames a second, 0 for the display's rate.
    fps: u32,
    /// When the frame began.
    begun: Instant,
}

fn id() -> egui::Id {
    egui::Id::new("frame-pace")
}

/// Notes the frame rate (`fps`, 0 for the display's) and that a frame begins; call it
/// at the start of each frame.
pub fn begin(ctx: &egui::Context, fps: u32) {
    let pace = Pace {
        fps,
        begun: Instant::now(),
    };
    ctx.data_mut(|d| d.insert_temp(id(), pace));
}

/// Asks for the next frame of an animation, as soon as the frame rate allows. Callable
/// from any thread (a battle's, for a new turn).
pub fn next_frame(ctx: &egui::Context) {
    next_frame_in(ctx, Duration::ZERO);
}

/// Asks for a frame once `wait` has passed, or later when the frame rate wants a longer
/// wait.
pub fn next_frame_in(ctx: &egui::Context, wait: Duration) {
    let pace = ctx.data(|d| d.get_temp::<Pace>(id()));
    let now = Instant::now();
    let wait = match pace {
        Some(Pace { fps, begun }) if fps > 0 => {
            let next = begun + Duration::from_secs_f64(1.0 / f64::from(fps));
            wait.max(next.saturating_duration_since(now))
        }
        _ => wait,
    };
    // egui takes the frame time it predicts off a delay, so as to draw by then: put it
    // back, or a wait of one frame comes out as none. No wait at all asks for a single
    // frame (`request_repaint` asks for two).
    let delay = if wait.is_zero() {
        Duration::from_nanos(1)
    } else {
        let predicted = ctx.input(|i| i.predicted_dt);
        wait + Duration::try_from_secs_f32(predicted).unwrap_or_default()
    };
    ctx.request_repaint_after(delay);
}

/// The options' name for a frame rate.
pub fn label(fps: u32) -> String {
    match fps {
        0 => "Display".to_string(),
        n => n.to_string(),
    }
}
