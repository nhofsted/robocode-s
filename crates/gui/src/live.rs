// SPDX-License-Identifier: EPL-2.0

//! A battle running locally: the server and robot processes on a background thread
//! (`runner::local`), observed by the GUI.

use crate::graphs::History;
use crate::input::Interaction;
use crate::screen::{self, DebugProperties};
use crate::sound::{self, Cue};
use crate::view::Camera;
use eframe::egui;
use robocode_s_protocol::v1::{self, server_message};
use robocode_s_runner::JavaHost;
use robocode_s_runner::local::{LocalBattle, LocalOutcome, run_local_battle};
use robocode_s_server::BattleOutcome;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// How much console output is kept per robot.
const OUTPUT_LIMIT: usize = 64 * 1024;
/// How many sound effects wait to be played at most (when nothing plays them).
const CUES_LIMIT: usize = 256;

enum Update {
    Message(Box<server_message::Msg>),
    Done(Result<LocalOutcome, String>),
}

pub struct Live {
    updates: Receiver<Update>,
    controls: Sender<v1::ObserverControl>,
    pub robots: Vec<v1::RobotStatics>,
    pub width: f64,
    pub height: f64,
    /// How far the sentry border reaches into the field.
    pub sentry_border: f64,
    pub rounds: i32,
    pub seed: i64,
    pub latest: Option<v1::TurnSnapshot>,
    /// The snapshot before `latest`, for the scan arcs.
    pub previous: Option<v1::TurnSnapshot>,
    /// Every turn so far, for the graphs and scores.
    pub history: History,
    /// Console output per robot.
    pub output: Vec<String>,
    /// Debug properties per robot, as they are now.
    pub properties: Vec<DebugProperties>,
    pub paint: Vec<bool>,
    pub paused: bool,
    pub turns_per_second: u32,
    /// Stop was pressed: the battle ends.
    pub stopping: bool,
    /// The end of the battle with its results, or why it failed.
    pub outcome: Option<Result<BattleOutcome, String>>,
    /// Mouse and keyboard on the battlefield, for interactive robots.
    pub interaction: Interaction,
    /// The battlefield view's zoom and pan.
    pub camera: Camera,
    /// The sound effects of the turns that arrived, until played (see `sound`).
    pub cues: Vec<Cue>,
}

impl Live {
    pub fn start(ctx: &egui::Context, host: JavaHost, battle: LocalBattle) -> Self {
        let (updates_tx, updates) = mpsc::channel();
        let (controls, controls_rx) = mpsc::channel();
        let robots: Vec<v1::RobotStatics> =
            battle.robots.iter().map(|r| r.statics.clone()).collect();
        let rules = &battle.config.rules;
        let (width, height, rounds) = (
            f64::from(rules.width),
            f64::from(rules.height),
            rules.num_rounds,
        );
        let sentry_border = f64::from(rules.sentry_border_size);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let tx = updates_tx.clone();
            let repaint = ctx.clone();
            let result = run_local_battle(&host, &battle, Some(controls_rx), |msg| {
                let _ = tx.send(Update::Message(Box::new(msg.clone())));
                repaint.request_repaint();
            });
            let _ = updates_tx.send(Update::Done(result));
            ctx.request_repaint();
        });
        let n = robots.len();
        Self {
            updates,
            controls,
            robots,
            width,
            height,
            sentry_border,
            rounds,
            seed: 0,
            latest: None,
            previous: None,
            history: History::new(n),
            output: vec![String::new(); n],
            properties: vec![DebugProperties::new(); n],
            paint: vec![false; n],
            paused: false,
            turns_per_second: 30,
            stopping: false,
            outcome: None,
            interaction: Interaction::default(),
            camera: Camera::default(),
            cues: Vec::new(),
        }
        .with_initial_speed()
    }

    fn with_initial_speed(self) -> Self {
        self.control(v1::observer_control::Control::TurnsPerSecond(
            self.turns_per_second,
        ));
        self
    }

    /// Takes what arrived since the last frame.
    pub fn poll(&mut self) {
        while let Ok(update) = self.updates.try_recv() {
            let update = match update {
                Update::Message(msg) => Ok(*msg),
                Update::Done(result) => Err(result),
            };
            match update {
                Ok(server_message::Msg::TurnSnapshot(s)) => {
                    for (out, r) in self.output.iter_mut().zip(&s.robots) {
                        if !r.output.is_empty() {
                            out.push_str(&r.output);
                            if out.len() > OUTPUT_LIMIT {
                                let cut = out.len() - OUTPUT_LIMIT;
                                let cut = (cut..out.len())
                                    .find(|&i| out.is_char_boundary(i))
                                    .unwrap_or(out.len());
                                out.drain(..cut);
                            }
                        }
                    }
                    for (properties, r) in self.properties.iter_mut().zip(&s.robots) {
                        screen::apply_debug_properties(properties, &r.debug_properties);
                    }
                    self.history.push(&s);
                    if self.cues.len() < CUES_LIMIT {
                        self.cues.extend(sound::cues(&s, self.width));
                    }
                    self.previous = self.latest.replace(s);
                }
                Ok(_) => {}
                Err(result) => {
                    self.outcome = Some(result.map(|o| o.outcome));
                }
            }
        }
    }

    pub fn control(&self, control: v1::observer_control::Control) {
        let _ = self.controls.send(v1::ObserverControl {
            control: Some(control),
        });
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.control(v1::observer_control::Control::Pause(paused));
    }

    /// Stops the battle (classic's stop): it ends with the results so far.
    pub fn stop(&mut self) {
        if !self.stopping && self.outcome.is_none() {
            self.stopping = true;
            self.control(v1::observer_control::Control::Abort(true));
        }
    }

    pub fn step(&self) {
        self.control(v1::observer_control::Control::Step(true));
    }

    pub fn set_speed(&mut self, turns_per_second: u32) {
        self.turns_per_second = turns_per_second;
        self.control(v1::observer_control::Control::TurnsPerSecond(
            turns_per_second,
        ));
    }

    /// Kills a robot (classic's "Kill robot"): it dies before the next turn.
    pub fn kill(&self, robot: usize) {
        if self.outcome.is_none() && !self.stopping {
            self.control(v1::observer_control::Control::Kill(robot as i32));
        }
    }

    /// A mouse or keyboard event on the battlefield: interactive robots get it with the
    /// next turn.
    pub fn input(&self, event: v1::InputEvent) {
        if self.outcome.is_none() && !self.stopping {
            self.control(v1::observer_control::Control::Input(event));
        }
    }

    pub fn set_paint(&mut self, robot: usize, enabled: bool) {
        self.paint[robot] = enabled;
        self.control(v1::observer_control::Control::Paint(v1::PaintToggle {
            robot_index: robot as i32,
            enabled,
        }));
    }
}

/// A battle that is closed (a new one, or a cancelled start) stops, and its robots with it.
impl Drop for Live {
    fn drop(&mut self) {
        if self.outcome.is_none() {
            self.control(v1::observer_control::Control::Abort(true));
        }
    }
}
