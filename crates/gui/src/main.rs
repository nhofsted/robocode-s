// SPDX-License-Identifier: EPL-2.0

//! `robocode-s-gui`: battle setup, a live viewer with debug paint, and replays
//! (design §10).

// No console window on Windows; started from a terminal, it writes there (`console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod backdrop;
#[cfg(windows)]
mod console;
mod files;
mod gpu;
mod graphs;
mod icons;
mod ide;
mod input;
mod live;
mod pace;
mod perf;
mod replay;
mod rerun;
mod rumble;
mod screen;
mod settings;
mod shell;
mod sound;
mod sprites;
mod track;
mod update;
mod view;
mod window;

use app::{App, Browse, Launch, Screenshot, Setup, parse_robots};
use robocode_s_runner::local::Environment;
use settings::Settings;
use shell::Tab;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: robocode-s-gui [recording] [--full]
       robocode-s-gui --battle ROBOTS [--rounds N] [--seed N] [--tps N] [--paint]
       robocode-s-gui --watch HOST:PORT [--token TOKEN] [--fingerprint F] [--tps N]

Without arguments: the battle setup. With a recording (from --record or
\"Record to\", or a website viewer track): its replay, in the Watch Battle tab.
With --full, a track downloaded from the website becomes a full recording at
once, as its Full recording button does: robots that aren't here yet come from
the rumble.
Robots come from the robot folders set in the options (the gear), saved in
$XDG_CONFIG_HOME/robocode-s/gui.json (~/.config/... by default; another file
with ROBOCODES_GUI_CONFIG): their jars, and the folders themselves when they hold
class files (an IDE's output, read in place: a recompiled robot is new in the
next battle). ROBOCODES_ROBOT_JARS adds jars and class folders (a path list).
The other paths come from
ROBOCODES_JAVA, ROBOCODES_HOST_LIB, ROBOCODES_AGENT_JAR and ROBOCODES_DATA, as
for `robocode-s battle`. ROBOCODES_OWN_FRAME=1 or 0 forces the app's own window
frame on or off (default: on, except on macOS).
It notes its start and exit, and a crash's message and backtrace, in
logs/robocode-s.log in the user data folder (%LOCALAPPDATA%\\robocode-s on
Windows, where stderr goes there too without a terminal; another file with
ROBOCODES_LOG). ROBOCODES_FRAME_LOG=1 logs the window's sizes and the moves and
resizes handed to the system to stderr. ROBOCODES_PERF_LOG=1 logs the renderer
and the frame rate and frame times once a second (=frames: every frame), and a
battle's controls (speed, pause, stop) as sent and as its server applies them, to
stderr. When the GPU goes away (the display sleeping or waking, a driver reset),
the window gives way to a new one and contributing to the rumble goes on; the log
says what was seen. ROBOCODES_LOSE_GPU=SECONDS loses the first window's GPU that
long after it opens, to try that.

  --watch HOST:PORT  watch the battle on that server as an observer (one run with
                     `robocode-s battle --observe PORT`), in the Watch Battle tab
  --token TOKEN      with --watch: the observer token ($ROBOCODES_OBSERVE_TOKEN)
  --fingerprint F    with --watch: the battle's certificate as it prints it
                     (sha256:AB:CD:...), for TLS; without one, a loopback server
                     is reached without TLS, another needs a CA's certificate
  --zoom Z[,X,Y]     battles and replays start zoomed in Z times (1 to 16), centred
                     on battlefield point X,Y (y up; default the field's centre)
  --tab NAME         open a tab at start: start, watch (or replay), rumble or options
  --browse FIELD     open the file dialog of a field at start: record, replay,
                     folder (the options' robot folder) or battle (a .battle file
                     to open), on its tab
  --size WxH         the window's size in points (default 1200x860, at least 720x480
                     except with --screenshot)
  --screenshot FILE  save the window as PNG and quit: in a battle at turn --after
                     of the first round (or its results, when it ends before),
                     otherwise after --after frames (default 120)
  --loading          with --screenshot and --battle or a replay: shoot the loading
                     screen after --after frames instead
  --from-ide         open the \"From your IDE...\" panel at start (for screenshots)
  --ide-add          add the robots started by an IDE to the battle as they connect
                     (for screenshots and testing)
  --rumble-demo STATE
                     show the Run Rumble tab in a made-up state, with no network
                     or sign-in (for screenshots): out, code, signed-in,
                     contributing, waiting or update
  --kill-at TURN:N  with --battle: kill robot N (from 0, in --battle's order) at
                     turn TURN of the first round, as its kill button does
  --input-at TURN:EVENT
                     with --battle: send a mouse or keyboard event to interactive
                     robots at turn TURN of the first round, as the battlefield
                     does (repeatable): move X,Y, drag X,Y, press X,Y[,BUTTON],
                     release X,Y[,BUTTON], click X,Y[,BUTTON], wheel X,Y,NOTCHES,
                     key NAME or keyup NAME (egui's key names: ArrowUp, W, ...);
                     battlefield coordinates, y up
";

/// The smallest window, in points, at which no page clips its controls: the title bar's
/// tabs and gear set the width; below the height the battlefield gets too small.
const MIN_SIZE: [f32; 2] = [720.0, 480.0];

/// The window's icon: classic Robocode's application icon, as the website's favicon and the
/// Windows program's own icon (`crates/gui/assets/README.md`).
const ICON: &[u8] = include_bytes!("../../../web/static/favicon-256.png");

fn icon() -> eframe::egui::IconData {
    let image = image::load_from_memory_with_format(ICON, image::ImageFormat::Png)
        .expect("the icon is a PNG")
        .into_rgba8();
    eframe::egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}

/// `--size`: `WxH` in points.
fn parse_size(text: &str) -> Option<[f32; 2]> {
    let (w, h) = text.split_once(['x', 'X'])?;
    let (w, h): (f32, f32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w >= 1.0 && h >= 1.0 && w.is_finite() && h.is_finite()).then_some([w, h])
}

/// `--zoom`: `Z` or `Z,X,Y` (the battlefield point at the view's centre).
fn parse_zoom(text: &str) -> Option<view::Camera> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse().ok().filter(|v: &f64| v.is_finite()))
        .collect::<Option<_>>()?;
    let (zoom, center) = match parts[..] {
        [z] => (z, None),
        [z, x, y] => (z, Some((x, y))),
        _ => return None,
    };
    (zoom >= 1.0).then_some(view::Camera {
        zoom: zoom.min(f64::from(view::MAX_ZOOM)) as f32,
        center,
    })
}

fn main() -> ExitCode {
    #[cfg(windows)]
    console::attach();
    update::wait_for_old();
    let settings = Settings::load();
    let mut setup = Setup::new(&settings);
    let (mut start, mut replay, mut shot, mut after) = (false, None, None, 120);
    let mut watch = false;
    if let Ok(token) = std::env::var("ROBOCODES_OBSERVE_TOKEN") {
        setup.watch_token = token;
    }
    let mut loading = false;
    let mut full = false;
    let mut tab = None;
    let mut browse = None;
    let mut kill_at = None;
    let (mut ide_panel, mut ide_add) = (false, false);
    let mut rumble_demo = None;
    let mut input_at = Vec::new();
    let mut camera = view::Camera::default();
    let mut size = [1200.0, 860.0];
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match arg.as_str() {
            "--battle" => {
                setup.robots = parse_robots(&value());
                start = true;
            }
            "--watch" => {
                setup.watch = value();
                watch = true;
            }
            "--token" => setup.watch_token = value(),
            "--fingerprint" => setup.watch_fingerprint = value(),
            "--rounds" => setup.rounds = value().parse().unwrap_or(setup.rounds),
            "--seed" => setup.seed = value(),
            "--tps" => setup.turns_per_second = value().parse().unwrap_or(30),
            "--paint" => setup.paint = true,
            "--record" => setup.record = value(),
            "--screenshot" => shot = Some(PathBuf::from(value())),
            "--after" => after = value().parse().unwrap_or(after),
            "--loading" => loading = true,
            "--full" => full = true,
            "--from-ide" => ide_panel = true,
            "--ide-add" => ide_add = true,
            "--rumble-demo" => rumble_demo = Some(value()),
            "--kill-at" => {
                let text = value();
                let parsed = text
                    .split_once(':')
                    .and_then(|(t, r)| Some((t.trim().parse().ok()?, r.trim().parse().ok()?)));
                let Some(k) = parsed else {
                    eprint!("--kill-at wants TURN:ROBOT, like 100:0, not {text:?}\n\n{USAGE}");
                    return ExitCode::from(2);
                };
                kill_at = Some(k);
            }
            "--input-at" => {
                let text = value();
                match input::parse_scripted(&text) {
                    Ok(scripted) => input_at.push(scripted),
                    Err(e) => {
                        eprint!("--input-at {text:?}: {e}\n\n{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
            "--zoom" => {
                let text = value();
                let Some(c) = parse_zoom(&text) else {
                    eprint!(
                        "--zoom wants Z or Z,X,Y, like 3 or 3,100,500, not {text:?}\n\n{USAGE}"
                    );
                    return ExitCode::from(2);
                };
                camera = c;
            }
            "--tab" => {
                let name = value();
                let Some(t) = Tab::parse(&name) else {
                    eprint!("--tab wants start, watch, rumble or options, not {name:?}\n\n{USAGE}");
                    return ExitCode::from(2);
                };
                tab = Some(t);
            }
            "--browse" => {
                let name = value();
                let (field, t) = match name.as_str() {
                    "record" => (Browse::Record, Tab::Start),
                    "replay" => (Browse::Replay, Tab::Watch),
                    "folder" => (Browse::Folder, Tab::Options),
                    "battle" => (Browse::OpenBattle, Tab::Start),
                    _ => {
                        eprint!(
                            "--browse wants record, replay, folder or battle, not {name:?}\n\n{USAGE}"
                        );
                        return ExitCode::from(2);
                    }
                };
                browse = Some(field);
                tab = tab.or(Some(t));
            }
            "--size" => {
                let text = value();
                let Some(s) = parse_size(&text) else {
                    eprint!("--size wants WxH, like 1200x860, not {text:?}\n\n{USAGE}");
                    return ExitCode::from(2);
                };
                size = s;
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            a if a.starts_with("--") => {
                eprint!("unknown option {a}\n\n{USAGE}");
                return ExitCode::from(2);
            }
            path => replay = Some(PathBuf::from(path)),
        }
    }
    // The log file (`rumble_client::log`), except for a screenshot's short run. Without a
    // terminal, the Windows build's stderr goes there too: an abort's last words.
    if shot.is_none() {
        rumble_client::log::init("robocode-s-gui");
        #[cfg(windows)]
        console::into_log();
    }
    // A screenshot may go below the minimum, to see where the layout clips.
    let min_size = match shot {
        Some(_) => [size[0].min(MIN_SIZE[0]), size[1].min(MIN_SIZE[1])],
        None => MIN_SIZE,
    };
    let screenshot = shot.map(|path| Screenshot::new(path, after, loading));
    let own_frame = window::wanted();
    let launch = Launch {
        setup,
        start,
        watch,
        replay,
        full,
        tab,
        browse,
        kill_at,
        input_at,
        camera,
        min_size: min_size.into(),
        ide_panel,
        ide_add,
        rumble_demo,
    };
    // A window that panics because the GPU went away (see `gpu`) gives way to a new one,
    // on a new device, with what the old one handed over: contributing goes on.
    let carry = app::Carry::default();
    let mut next = Some((launch, screenshot, settings));
    let mut restarts = Restarts::default();
    loop {
        let (launch, screenshot, settings) =
            next.take().unwrap_or_else(|| resumed(&carry, min_size));
        let options = native_options(size, min_size, own_frame);
        let for_app = carry.clone();
        let opened = Instant::now();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            eframe::run_native(
                "robocode-s",
                options,
                Box::new(move |cc| {
                    Ok(Box::new(App::new(
                        cc,
                        Environment::from_env(),
                        settings,
                        launch,
                        screenshot,
                        own_frame,
                        for_app,
                    )))
                }),
            )
        }));
        let (why, payload) = match result {
            Ok(Ok(())) => {
                rumble_client::log::exit("the window closed");
                return ExitCode::SUCCESS;
            }
            Ok(Err(e)) if restarts.count == 0 => {
                rumble_client::log::exit(&format!("failed: {e}"));
                eprintln!("robocode-s-gui: {e}");
                return ExitCode::FAILURE;
            }
            // A new window that couldn't start: the GPU may not be back yet.
            Ok(Err(e)) => (format!("the new window failed: {e}"), None),
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                if gpu::after_panic(&message).is_none() {
                    std::panic::resume_unwind(payload);
                }
                if !cfg!(windows) {
                    // winit 0.30 on X11 and Wayland keeps "running" set after a panic, so
                    // its event loop never starts a new window; only Windows resets it.
                    rumble_client::log::line(
                        "gpu: no new window: winit can't run again after a panic here",
                    );
                    std::panic::resume_unwind(payload);
                }
                if carry.borrow().rumble.is_none() && rumble::contributing() {
                    // The window didn't hand the page over: a new one would start a
                    // second contributor beside the first.
                    rumble_client::log::line(
                        "gpu: no new window: the old one didn't hand over the contributing thread",
                    );
                    std::panic::resume_unwind(payload);
                }
                ("the GPU went away".to_string(), Some(payload))
            }
        };
        let Some(wait) = restarts.next(opened.elapsed()) else {
            rumble_client::log::line(&format!(
                "gpu: {why}; no new window after {} tries",
                restarts.count - 1
            ));
            match payload {
                Some(payload) => std::panic::resume_unwind(payload),
                None => {
                    rumble_client::log::exit(&format!("failed: {why}"));
                    return ExitCode::FAILURE;
                }
            }
        };
        rumble_client::log::line(&format!(
            "gpu: {why} after {:.0} s of the window; a new window in {} s (try {}), \
             contributing: {}",
            opened.elapsed().as_secs_f64(),
            wait.as_secs(),
            restarts.count,
            if rumble::contributing() {
                "yes, it goes on"
            } else {
                "no"
            }
        ));
        std::thread::sleep(wait);
    }
}

fn native_options(size: [f32; 2], min_size: [f32; 2], own_frame: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("robocode-s")
            // The Wayland app id and X11 class, which desktops match to `robocode-s.desktop`.
            .with_app_id("robocode-s")
            .with_icon(icon())
            .with_inner_size(size)
            .with_min_inner_size(min_size)
            .with_decorations(!own_frame),
        ..Default::default()
    }
}

/// What a new window starts with: what the old one handed over (the Run Rumble page is
/// taken by [`App::new`]), and none of the command line's one-time actions.
fn resumed(carry: &app::Carry, min_size: [f32; 2]) -> (Launch, Option<Screenshot>, Settings) {
    let mut carried = carry.borrow_mut();
    let settings = carried.settings.take().unwrap_or_else(Settings::load);
    let setup = carried
        .setup
        .take()
        .unwrap_or_else(|| Setup::new(&settings));
    let launch = Launch {
        setup,
        start: false,
        watch: false,
        replay: None,
        full: false,
        tab: carried.tab.take(),
        browse: None,
        kill_at: None,
        input_at: Vec::new(),
        camera: view::Camera::default(),
        min_size: min_size.into(),
        ide_panel: false,
        ide_add: false,
        rumble_demo: None,
    };
    (launch, None, settings)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// How often a window that lost the GPU is replaced: after 2, 4, 8 ... 60 s, at most
/// [`Restarts::MAX`] times in a row; a window that lasted [`Restarts::RESET`] starts the
/// count again.
#[derive(Default)]
struct Restarts {
    count: u32,
}

impl Restarts {
    const MAX: u32 = 8;
    const RESET: Duration = Duration::from_secs(600);

    /// The wait before the next window, after one that lasted `lived`; none: give up.
    fn next(&mut self, lived: Duration) -> Option<Duration> {
        if lived >= Self::RESET {
            self.count = 0;
        }
        self.count += 1;
        (self.count <= Self::MAX).then(|| Duration::from_secs((1u64 << self.count).min(60)))
    }
}

#[cfg(test)]
mod tests {
    use super::Restarts;
    use std::time::Duration;

    #[test]
    fn restarts_back_off_give_up_and_start_again() {
        let mut r = Restarts::default();
        let short = Duration::from_secs(5);
        let waits: Vec<u64> = (0..Restarts::MAX)
            .map(|_| r.next(short).unwrap().as_secs())
            .collect();
        assert_eq!(waits, [2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(r.next(short), None);
        // A window that lasted counts as a first loss again.
        assert_eq!(r.next(Restarts::RESET), Some(Duration::from_secs(2)));
    }

    #[test]
    fn the_icon_decodes() {
        let icon = super::icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    /// The .exe's icon (`build.rs`) holds the window's icon as its 256 pixel image.
    #[test]
    fn the_exe_icon_matches_the_window_icon() {
        let ico = include_bytes!("../assets/robocode-s.ico");
        let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap()) as usize;
        let sizes: Vec<usize> = (0..u16_at(4))
            .map(|i| match ico[6 + 16 * i] {
                0 => 256,
                w => w as usize,
            })
            .collect();
        assert_eq!(sizes, [16, 24, 32, 48, 64, 128, 256]);
        let last = 6 + 16 * 6;
        let (len, offset) = (u32_at(last + 8), u32_at(last + 12));
        assert_eq!(&ico[offset..offset + len], super::ICON);
    }

    use super::{parse_size, parse_zoom};

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1200x860"), Some([1200.0, 860.0]));
        assert_eq!(parse_size("800X600"), Some([800.0, 600.0]));
        assert_eq!(parse_size("800"), None);
        assert_eq!(parse_size("0x600"), None);
        assert_eq!(parse_size("ax600"), None);
    }

    #[test]
    fn zooms() {
        let c = parse_zoom("3").unwrap();
        assert_eq!((c.zoom, c.center), (3.0, None));
        let c = parse_zoom("2.5, 100,500").unwrap();
        assert_eq!((c.zoom, c.center), (2.5, Some((100.0, 500.0))));
        assert_eq!(parse_zoom("99").unwrap().zoom, 16.0);
        assert_eq!(parse_zoom("0.5"), None);
        assert_eq!(parse_zoom("2,1"), None);
        assert_eq!(parse_zoom("x"), None);
    }
}
