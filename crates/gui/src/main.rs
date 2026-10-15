// SPDX-License-Identifier: EPL-2.0

//! `robocode-s-gui`: battle setup, a live viewer with debug paint, and replays
//! (design §10).

mod app;
mod live;
mod replay;
mod sprites;
mod view;
mod window;

use app::{App, Screenshot, Setup};
use robocode_s_runner::local::Environment;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: robocode-s-gui [recording]
       robocode-s-gui --battle ROBOTS [--rounds N] [--seed N] [--tps N] [--paint]

Without arguments: the battle setup. With a recording: its replay.
Paths come from ROBOCODES_JAVA, ROBOCODES_HOST_LIB, ROBOCODES_AGENT_JAR,
ROBOCODES_DATA and ROBOCODES_ROBOT_JARS, as for `robocode-s battle`.
ROBOCODES_OWN_FRAME=1 or 0 forces the app's own window frame on or off (default:
on under WSLg, whose compositor crashes on the system's frame).

  --screenshot FILE  save the window as PNG and quit: in a battle at turn --after
                     of the first round, otherwise after --after frames (default 120)
";

fn main() -> ExitCode {
    let mut setup = Setup::default();
    let (mut start, mut replay, mut shot, mut after) = (false, None, None, 120);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match arg.as_str() {
            "--battle" => {
                setup.robots = value();
                start = true;
            }
            "--rounds" => setup.rounds = value().parse().unwrap_or(setup.rounds),
            "--seed" => setup.seed = value(),
            "--tps" => setup.turns_per_second = value().parse().unwrap_or(30),
            "--paint" => setup.paint = true,
            "--record" => setup.record = value(),
            "--screenshot" => shot = Some(PathBuf::from(value())),
            "--after" => after = value().parse().unwrap_or(after),
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
    let screenshot = shot.map(|path| Screenshot::new(path, after));
    let own_frame = window::wanted();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("robocode-s")
            .with_inner_size([1200.0, 860.0])
            .with_min_inner_size([760.0, 540.0])
            .with_decorations(!own_frame),
        ..Default::default()
    };
    let result = eframe::run_native(
        "robocode-s",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                cc,
                Environment::from_env(),
                setup,
                start,
                replay,
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
