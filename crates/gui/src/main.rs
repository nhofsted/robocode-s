// SPDX-License-Identifier: EPL-2.0

//! `robocode-s-gui`: battle setup, a live viewer with debug paint, and replays
//! (design §10).

mod app;
mod backdrop;
mod files;
mod graphs;
mod icons;
mod input;
mod live;
mod perf;
mod replay;
mod screen;
mod settings;
mod shell;
mod sound;
mod sprites;
mod track;
mod view;
mod window;

use app::{App, Browse, Launch, Screenshot, Setup, parse_robots};
use robocode_s_runner::local::Environment;
use settings::Settings;
use shell::Tab;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: robocode-s-gui [recording]
       robocode-s-gui --battle ROBOTS [--rounds N] [--seed N] [--tps N] [--paint]
       robocode-s-gui --watch HOST:PORT [--token TOKEN] [--tps N]

Without arguments: the battle setup. With a recording (from --record or
\"Record to\", or a website viewer track): its replay, in the Watch Battle tab.
Robots come from the robot folders set in the options (the gear), saved in
$XDG_CONFIG_HOME/robocode-s/gui.json (~/.config/... by default; another file
with ROBOCODES_GUI_CONFIG): their jars, and the folders themselves when they hold
class files (an IDE's output, read in place: a recompiled robot is new in the
next battle). ROBOCODES_ROBOT_JARS adds jars and class folders (a path list).
The other paths come from
ROBOCODES_JAVA, ROBOCODES_HOST_LIB, ROBOCODES_AGENT_JAR and ROBOCODES_DATA, as
for `robocode-s battle`. ROBOCODES_OWN_FRAME=1 or 0 forces the app's own window
frame on or off (default: on under WSLg, whose compositor crashes on the
system's frame). ROBOCODES_FRAME_LOG=1 logs the window's sizes and the moves and
resizes handed to the system to stderr. ROBOCODES_PERF_LOG=1 logs the renderer
and the frame rate and frame times once a second (=frames: every frame) to stderr.

  --watch HOST:PORT  watch the battle on that server as an observer (one run with
                     `robocode-s battle --observe PORT`), in the Watch Battle tab
  --token TOKEN      with --watch: the observer token ($ROBOCODES_OBSERVE_TOKEN)
  --zoom Z[,X,Y]     battles and replays start zoomed in Z times (1 to 16), centred
                     on battlefield point X,Y (y up; default the field's centre)
  --tab NAME         open a tab at start: start, watch (or replay) or options
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
  --kill-at TURN:N   with --battle: kill robot N (from 0, in --battle's order) at
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
    let settings = Settings::load();
    let mut setup = Setup::new(&settings);
    let (mut start, mut replay, mut shot, mut after) = (false, None, None, 120);
    let mut watch = false;
    if let Ok(token) = std::env::var("ROBOCODES_OBSERVE_TOKEN") {
        setup.watch_token = token;
    }
    let mut loading = false;
    let mut tab = None;
    let mut browse = None;
    let mut kill_at = None;
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
            "--rounds" => setup.rounds = value().parse().unwrap_or(setup.rounds),
            "--seed" => setup.seed = value(),
            "--tps" => setup.turns_per_second = value().parse().unwrap_or(30),
            "--paint" => setup.paint = true,
            "--record" => setup.record = value(),
            "--screenshot" => shot = Some(PathBuf::from(value())),
            "--after" => after = value().parse().unwrap_or(after),
            "--loading" => loading = true,
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
                    eprint!("--tab wants start, watch or options, not {name:?}\n\n{USAGE}");
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
    // A screenshot may go below the minimum, to see where the layout clips.
    let min_size = match shot {
        Some(_) => [size[0].min(MIN_SIZE[0]), size[1].min(MIN_SIZE[1])],
        None => MIN_SIZE,
    };
    let screenshot = shot.map(|path| Screenshot::new(path, after, loading));
    let own_frame = window::wanted();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("robocode-s")
            .with_inner_size(size)
            .with_min_inner_size(min_size)
            .with_decorations(!own_frame),
        ..Default::default()
    };
    let result = eframe::run_native(
        "robocode-s",
        options,
        Box::new(move |cc| {
            let launch = Launch {
                setup,
                start,
                watch,
                replay,
                tab,
                browse,
                kill_at,
                input_at,
                camera,
                min_size: min_size.into(),
            };
            Ok(Box::new(App::new(
                cc,
                Environment::from_env(),
                settings,
                launch,
                screenshot,
                own_frame,
            )))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("robocode-s-gui: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
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
