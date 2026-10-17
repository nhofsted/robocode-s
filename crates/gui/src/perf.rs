// SPDX-License-Identifier: EPL-2.0

//! `ROBOCODES_PERF_LOG=1`: the renderer the window got, and how long its frames take, on
//! stderr. Once a second (while it repaints) a line with the frames drawn, how many of
//! them at a new window size, the time between them, the app's own time per frame
//! (laying out the pages) and eframe's CPU time per frame (the app and tessellation;
//! without submitting and presenting the frame, which is where a software renderer such
//! as llvmpipe draws it). `ROBOCODES_PERF_LOG=frames` adds a line per frame: when it
//! began (milliseconds of the wall clock's last 100 seconds, to line it up with other
//! logs), its size, its times and what asked for it. The once-a-second line also has the
//! live battle's turns a second: as they arrived from the server, and as the window took
//! them in.

use eframe::egui;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Turns of live battles that arrived from their servers, and that the window took in.
static ARRIVED: AtomicU64 = AtomicU64::new(0);
static TAKEN: AtomicU64 = AtomicU64::new(0);

/// Counts a live battle's turn as it arrives (on the battle's thread).
pub fn turn_arrived() {
    ARRIVED.fetch_add(1, Ordering::Relaxed);
}

/// Counts a live battle's turn as the window takes it in.
pub fn turn_taken() {
    TAKEN.fetch_add(1, Ordering::Relaxed);
}

/// What the log adds up between its lines.
pub struct Perf {
    every_frame: bool,
    /// When the frame being laid out began, and when the one before did.
    begun: Option<Instant>,
    previous: Option<Instant>,
    /// Since the last line: frames, and the sums and maxima of their times (seconds).
    window: Instant,
    frames: u32,
    resized: u32,
    size: egui::Vec2,
    gap: (f64, f64),
    app: (f64, f64),
    cpu: (f64, f64),
}

impl Perf {
    /// The log when `ROBOCODES_PERF_LOG` asks for it.
    pub fn from_env() -> Option<Self> {
        let every_frame = match std::env::var("ROBOCODES_PERF_LOG").ok()?.as_str() {
            "" | "0" => return None,
            "frames" => true,
            _ => false,
        };
        Some(Self {
            every_frame,
            begun: None,
            previous: None,
            window: Instant::now(),
            frames: 0,
            resized: 0,
            size: egui::Vec2::ZERO,
            gap: (0.0, 0.0),
            app: (0.0, 0.0),
            cpu: (0.0, 0.0),
        })
    }

    /// Logs the renderer: the adapter wgpu chose, and the others it found.
    pub fn renderer(cc: &eframe::CreationContext) {
        let Some(state) = &cc.wgpu_render_state else {
            eprintln!("[perf] no wgpu render state");
            return;
        };
        let describe = |info: &eframe::wgpu::AdapterInfo| {
            format!(
                "{} ({:?}, {:?}, driver {} {})",
                info.name, info.backend, info.device_type, info.driver, info.driver_info
            )
        };
        eprintln!("[perf] adapter: {}", describe(&state.adapter.get_info()));
        for adapter in state.available_adapters.iter() {
            eprintln!("[perf] available: {}", describe(&adapter.get_info()));
        }
        eprintln!(
            "[perf] surface format {:?}, present mode {:?}, frame latency {:?}",
            state.target_format,
            state.surface_config.present_mode,
            state.surface_config.desired_maximum_frame_latency
        );
    }

    /// Call at the start of the app's frame.
    pub fn begin(&mut self) {
        let now = Instant::now();
        self.previous = self.begun.replace(now);
    }

    /// Call at the end of the app's frame.
    pub fn end(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let (Some(begun), now) = (self.begun, Instant::now()) else {
            return;
        };
        let app = (now - begun).as_secs_f64();
        let gap = self.previous.map(|p| (begun - p).as_secs_f64());
        // eframe's time for the frame before this one.
        let cpu = frame.info().cpu_usage.map(f64::from);
        let add = |sum: &mut (f64, f64), v: f64| {
            sum.0 += v;
            sum.1 = sum.1.max(v);
        };
        let size = ctx.input(|i| i.content_rect().size()) * ctx.pixels_per_point();
        self.frames += 1;
        if size != self.size {
            self.size = size;
            self.resized += 1;
        }
        add(&mut self.app, app);
        if let Some(gap) = gap {
            add(&mut self.gap, gap);
        }
        if let Some(cpu) = cpu {
            add(&mut self.cpu, cpu);
        }
        if self.every_frame {
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| (d.as_secs_f64() % 100.0) * 1e3 - app * 1e3);
            let causes: Vec<String> = ctx
                .repaint_causes()
                .iter()
                .map(ToString::to_string)
                .collect();
            let ms = |t: Option<f64>| t.map_or("-".into(), |t| format!("{:.1} ms", t * 1e3));
            eprintln!(
                "[perf] {at:.1} frame {:.0}x{:.0} px: gap {}, app {:.1} ms, eframe cpu (last) {}, asked by {}",
                size.x,
                size.y,
                ms(gap),
                app * 1e3,
                ms(cpu),
                causes.join(", "),
            );
        }
        let span = (now - self.window).as_secs_f64();
        if span >= 1.0 {
            let n = f64::from(self.frames);
            let arrived = ARRIVED.swap(0, Ordering::Relaxed) as f64;
            let taken = TAKEN.swap(0, Ordering::Relaxed) as f64;
            eprintln!(
                "[perf] {:.0}x{:.0} px: {:.1} fps ({} resized), gap {:.1} ms (max {:.1}), app {:.1} ms (max {:.1}), eframe cpu {:.1} ms (max {:.1}), turns {:.0}/s arrived, {:.0}/s shown",
                size.x,
                size.y,
                n / span,
                self.resized,
                self.gap.0 / n * 1e3,
                self.gap.1 * 1e3,
                self.app.0 / n * 1e3,
                self.app.1 * 1e3,
                self.cpu.0 / n * 1e3,
                self.cpu.1 * 1e3,
                arrived / span,
                taken / span,
            );
            self.window = now;
            self.frames = 0;
            self.resized = 0;
            self.gap = (0.0, 0.0);
            self.app = (0.0, 0.0);
            self.cpu = (0.0, 0.0);
        }
    }
}
