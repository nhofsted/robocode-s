// SPDX-License-Identifier: EPL-2.0

//! Playing a recording back (design §11): the engine re-runs the recorded inputs.
//! Opening a recording runs it once and keeps a keyframe (a clone of the replay) every
//! [`KEYFRAME`] turns, so any turn can be shown by replaying at most that many. That run
//! takes a while for a long battle, so it happens on a thread of its own ([`Opening`]).
//!
//! It also plays the website's lossy viewer tracks (see [`crate::track`]): one round, every
//! turn decoded up front, without scores, CPU times, debug paint or console output.

use crate::graphs::History;
use crate::icons;
use crate::screen::{self, DebugProperties};
use crate::shell;
use crate::sound;
use crate::sprites::Sprites;
use crate::track::{self, TrackBattle};
use crate::view::{self, Camera, Field, Style};
use eframe::egui::{self, Color32, RichText};
use robocode_s_protocol::v1;
use robocode_s_server::recording::{Frame, Recording, Replay, engine_warning, round_result};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

const KEYFRAME: usize = 200;
/// Turns a second at the speed slider's "max".
const FASTEST: f64 = 10_000.0;

pub struct ReplayView {
    title: String,
    robots: Vec<v1::RobotStatics>,
    width: f64,
    height: f64,
    sentry_border: f64,
    rounds: i32,
    /// Where the turns come from.
    source: Source,
    /// Index of the first turn of each round.
    round_starts: Vec<usize>,
    /// Every turn, for the graphs and scores.
    history: History,
    /// Console output: (turn index, robot, text).
    output: Vec<(usize, usize, String)>,
    /// Debug property changes: (turn index, robot, changes).
    properties: Vec<(usize, usize, Vec<v1::DebugProperty>)>,
    turns: usize,
    unverified: usize,
    /// Set when it was recorded with another engine version than this one.
    engine_warning: Option<String>,
    /// A track's rumble battle, when the website appended it, and the track's gun cooling
    /// rate in thousandths: it can run again ([`crate::rerun`]).
    battle: Option<(TrackBattle, i64)>,
    /// Recorded in a debug battle (robots from an IDE, not timed).
    debug: bool,
    results: Vec<v1::Results>,
    /// The shown turn.
    position: usize,
    current: Option<v1::TurnSnapshot>,
    /// The turn before the shown one, for the scan arcs.
    previous: Option<v1::TurnSnapshot>,
    playing: bool,
    /// Turns a second; 0 for as fast as possible.
    turns_per_second: u32,
    last_tick: Option<Instant>,
    paint_visible: Vec<bool>,
    /// Per robot, the turn index its output was cleared at: output up to it is not shown.
    cleared: Vec<Option<usize>>,
    /// The results window was closed at the last turn; it shows again there after a seek.
    results_closed: bool,
    /// The battlefield view's zoom and pan.
    pub camera: Camera,
    /// The sound effects of the turns played, until played (see `sound`).
    pub cues: Vec<sound::Cue>,
}

enum Source {
    /// A full recording, re-run by the engine.
    Recording {
        seed: i64,
        /// Replays positioned so that their next snapshot is turn `KEYFRAME * k`.
        keyframes: Vec<Replay>,
        /// The replay positioned after the shown turn.
        cursor: Box<Replay>,
    },
    /// A viewer track's turns.
    Track(Vec<v1::TurnSnapshot>),
}

/// The next snapshot of a replay, skipping round ends.
fn next_snapshot(replay: &mut Replay) -> Option<(v1::TurnSnapshot, bool)> {
    replay.find_map(|f| match f {
        Frame::Snapshot { snapshot, verified } => Some((snapshot, verified)),
        Frame::RoundOver(_) => None,
    })
}

/// A recording being opened on a thread of its own: its progress, and the view once it is
/// there. Dropping it cancels the opening.
pub struct Opening {
    pub path: PathBuf,
    /// How far the replay got, in millionths.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<ReplayView, String>>,
}

impl Opening {
    pub fn start(ctx: &egui::Context, path: PathBuf) -> Self {
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, done) = mpsc::channel();
        {
            let (path, progress, cancel) = (path.clone(), progress.clone(), cancel.clone());
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let report = |fraction: f64| {
                    progress.store((fraction * 1e6) as u32, Ordering::Relaxed);
                };
                let view = ReplayView::open(&path, &report, &cancel);
                let _ = tx.send(view);
                ctx.request_repaint();
            });
        }
        Self {
            path,
            progress,
            cancel,
            done,
        }
    }

    /// How far it got, from 0 to 1.
    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1e6
    }

    /// The view or why it couldn't be opened, once that is known.
    pub fn poll(&self) -> Option<Result<ReplayView, String>> {
        match self.done.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the replay stopped".into())),
        }
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ReplayView {
    /// Opens a full recording or a viewer track, told apart by their first bytes.
    /// `progress` hears how far the replay got (0 to 1); a `cancel` stops it.
    pub fn open(path: &Path, progress: &dyn Fn(f64), cancel: &AtomicBool) -> Result<Self, String> {
        let mut head = [0u8; 2];
        let read = std::fs::File::open(path)
            .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
            .map_err(|e| e.to_string())?;
        if track::is_track(&head[..read]) {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            return Ok(Self::track(path, track::decode(&bytes)?));
        }
        let recording = Recording::read(path).map_err(|e| e.to_string())?;
        let mut replay = recording.replay();
        let (mut keyframes, mut round_starts, mut output) = (Vec::new(), Vec::new(), Vec::new());
        let mut properties = Vec::new();
        let mut history = History::new(recording.header.robots.len());
        let (mut turns, mut unverified, mut round) = (0usize, 0usize, -1);
        let entries = recording.entries.len().max(1);
        loop {
            if turns % KEYFRAME == 0 {
                keyframes.push(replay.clone());
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                progress(replay.position() as f64 / entries as f64);
            }
            let Some((snapshot, verified)) = next_snapshot(&mut replay) else {
                break;
            };
            if snapshot.round != round {
                round = snapshot.round;
                round_starts.push(turns);
            }
            for (i, r) in snapshot.robots.iter().enumerate() {
                if !r.output.is_empty() {
                    output.push((turns, i, r.output.clone()));
                }
                if !r.debug_properties.is_empty() {
                    properties.push((turns, i, r.debug_properties.clone()));
                }
            }
            history.push(&snapshot);
            unverified += usize::from(!verified);
            turns += 1;
        }
        let results = round_result(replay.battle()).results;
        let rules = recording.header.rules.unwrap_or_default();
        let robots = recording.header.robots.clone();
        let mut view = Self {
            title: title(path),
            paint_visible: vec![true; robots.len()],
            cleared: vec![None; robots.len()],
            robots,
            width: f64::from(rules.battlefield_width),
            height: f64::from(rules.battlefield_height),
            sentry_border: f64::from(rules.sentry_border_size),
            rounds: rules.num_rounds,
            source: Source::Recording {
                seed: recording.header.seed,
                cursor: Box::new(keyframes[0].clone()),
                keyframes,
            },
            round_starts,
            history,
            output,
            properties,
            turns,
            unverified,
            engine_warning: recording.engine_warning(),
            battle: None,
            debug: recording.header.debug,
            results,
            position: 0,
            current: None,
            previous: None,
            playing: true,
            turns_per_second: 30,
            last_tick: None,
            results_closed: false,
            camera: Camera::default(),
            cues: Vec::new(),
        };
        view.seek(0);
        Ok(view)
    }

    fn track(path: &Path, decoded: track::Decoded) -> Self {
        let mut history = History::new(decoded.robots.len());
        for snapshot in &decoded.snapshots {
            history.push(snapshot);
        }
        let mut view = Self {
            title: title(path),
            paint_visible: vec![true; decoded.robots.len()],
            cleared: vec![None; decoded.robots.len()],
            robots: decoded.robots,
            width: decoded.width,
            sentry_border: 0.0,
            height: decoded.height,
            rounds: 1,
            turns: decoded.snapshots.len(),
            source: Source::Track(decoded.snapshots),
            round_starts: vec![0],
            history,
            output: Vec::new(),
            properties: Vec::new(),
            unverified: 0,
            engine_warning: engine_warning(decoded.engine_version),
            battle: decoded.battle.map(|b| (b, decoded.gun_cooling_milli)),
            debug: false,
            results: Vec::new(),
            position: 0,
            current: None,
            previous: None,
            playing: true,
            turns_per_second: 30,
            last_tick: None,
            results_closed: false,
            camera: Camera::default(),
            cues: Vec::new(),
        };
        view.seek(0);
        view
    }

    /// A track's rumble battle and the track's gun cooling rate in thousandths (see
    /// [`ReplayView::battle`]).
    pub fn battle(&self) -> Option<&(TrackBattle, i64)> {
        self.battle.as_ref()
    }

    /// A full recording: with scores, CPU times, debug paint and console output.
    fn full(&self) -> bool {
        matches!(self.source, Source::Recording { .. })
    }

    /// Shows turn `index` (clamped).
    fn seek(&mut self, index: usize) {
        if self.turns == 0 {
            return;
        }
        let index = index.min(self.turns - 1);
        match &mut self.source {
            Source::Track(snapshots) => {
                self.previous = index.checked_sub(1).map(|i| snapshots[i].clone());
                self.current = Some(snapshots[index].clone());
            }
            Source::Recording {
                keyframes, cursor, ..
            } => {
                if self.current.is_some() && index == self.position + 1 {
                    self.previous = self.current.take();
                } else {
                    // From the keyframe before the turn before, so that turn is known as well.
                    let before = index.saturating_sub(1);
                    **cursor = keyframes[before / KEYFRAME].clone();
                    for _ in 0..before % KEYFRAME {
                        next_snapshot(cursor);
                    }
                    self.previous = if index > 0 {
                        next_snapshot(cursor).map(|(s, _)| s)
                    } else {
                        None
                    };
                }
                self.current = next_snapshot(cursor).map(|(s, _)| s);
            }
        }
        self.position = index;
    }

    fn round_of(&self, index: usize) -> usize {
        self.round_starts
            .partition_point(|&s| s <= index)
            .saturating_sub(1)
    }

    /// Advances playback by wall-clock time.
    fn play(&mut self, ctx: &egui::Context) {
        if !self.playing || self.position + 1 >= self.turns {
            self.last_tick = None;
            return;
        }
        let now = Instant::now();
        let last = *self.last_tick.get_or_insert(now);
        let rate = match self.turns_per_second {
            0 => FASTEST,
            tps => f64::from(tps),
        };
        let due = (now - last).as_secs_f64() * rate;
        if due >= 1.0 {
            let steps = (due as usize).min(1000);
            let target = (self.position + steps).min(self.turns - 1);
            if target == self.position + 1 {
                self.seek(target);
                // Sound plays at a watchable speed only, a turn at a time.
                if (1..=sound::MAX_TPS).contains(&self.turns_per_second)
                    && let Some(s) = &self.current
                {
                    self.cues.extend(sound::cues(s, self.width));
                }
            } else {
                // Larger jumps: replay forward from here.
                while self.position < target {
                    let next = self.position + 1;
                    self.seek(next);
                }
            }
            self.last_tick = Some(last + std::time::Duration::from_secs_f64(steps as f64 / rate));
        }
        // The next frame when the next turn is due.
        let next = self.last_tick.unwrap_or(now) + std::time::Duration::from_secs_f64(1.0 / rate);
        crate::pace::next_frame_in(ctx, next.saturating_duration_since(Instant::now()));
    }

    /// Draws the replay on the battle screen (see [`screen`]); true when the user closes it.
    /// A robot's output to save goes in `save`; `run` is set when the user wants a track's
    /// battle run again.
    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        sprites: &mut Sprites,
        style: Style,
        selected: &mut usize,
        save: &mut Option<screen::SaveRequest>,
        run: &mut bool,
    ) -> bool {
        let ctx = &root.ctx().clone();
        self.play(ctx);
        let mut back = false;
        let current = self.current.clone();
        let full = self.full();
        screen::robots_panel(
            root,
            "replay-robots",
            &self.robots,
            current.as_ref(),
            &self.history,
            self.position + 1,
            selected,
            full,
        );
        // A track has no console output or debug paint.
        if full {
            let (output, properties) = (&self.output, &self.properties);
            let position = self.position;
            let cleared = &self.cleared;
            let event = screen::output_panel(
                root,
                "replay-output",
                &self.robots,
                current.as_ref(),
                selected,
                &self.paint_visible,
                |i| {
                    let from = cleared.get(i).copied().flatten();
                    let mut text: String = output
                        .iter()
                        .take_while(|(turn, _, _)| *turn <= position)
                        .filter(|(turn, robot, _)| {
                            *robot == i && from.is_none_or(|from| *turn > from)
                        })
                        .map(|(_, _, t)| t.as_str())
                        .collect();
                    // As much as a live battle shows.
                    screen::keep_output_tail(&mut text);
                    Cow::Owned(text)
                },
                |i| {
                    // As they were at the shown turn.
                    let mut shown = DebugProperties::new();
                    for (_, _, changes) in properties
                        .iter()
                        .take_while(|(turn, _, _)| *turn <= position)
                        .filter(|(_, robot, _)| *robot == i)
                    {
                        screen::apply_debug_properties(&mut shown, changes);
                    }
                    Cow::Owned(shown)
                },
                false,
            );
            match event {
                Some(screen::OutputEvent::TogglePaint(i)) => {
                    self.paint_visible[i] = !self.paint_visible[i];
                }
                Some(screen::OutputEvent::Save(i, text)) => {
                    *save = Some(screen::output_save(&self.robots[i], text));
                }
                // A replay can't change what happened.
                Some(screen::OutputEvent::Kill(_)) => {}
                Some(screen::OutputEvent::Clear(i)) => {
                    if let Some(c) = self.cleared.get_mut(i) {
                        *c = Some(position);
                    }
                }
                None => {}
            }
        }
        screen::controls_panel(root, "replay-controls", |ui| {
            // The timeline, as wide as the controls.
            let mut position = self.position;
            ui.spacing_mut().slider_width = ui.available_width();
            // Played so far in the app's accent, not egui's blue.
            ui.visuals_mut().selection.bg_fill = shell::ACCENT.gamma_multiply(0.8);
            let slider = ui.add(
                egui::Slider::new(&mut position, 0..=self.turns.saturating_sub(1))
                    .show_value(false)
                    .trailing_fill(true),
            );
            if slider.changed() {
                self.seek(position);
            }
            ui.horizontal(|ui| {
                if screen::control(ui, true, "Close the replay", |p, c, color| {
                    icons::stop(p, c, 6.0, color)
                }) {
                    back = true;
                }
                let playing = self.playing;
                let hint = if playing { "Pause" } else { "Play" };
                if screen::control(ui, true, hint, |p, c, color| {
                    if playing {
                        icons::pause(p, c, 6.0, color);
                    } else {
                        icons::play(p, c, 7.0, color);
                    }
                }) {
                    self.playing = !self.playing;
                    self.last_tick = None;
                }
                let at_start = self.position == 0;
                if screen::control(ui, !at_start, "Previous turn", |p, c, color| {
                    icons::step(p, c, 6.5, -1.0, color)
                }) {
                    self.playing = false;
                    self.seek(self.position.saturating_sub(1));
                }
                let at_end = self.position + 1 >= self.turns;
                if screen::control(ui, !at_end, "Next turn", |p, c, color| {
                    icons::step(p, c, 6.5, 1.0, color)
                }) {
                    self.playing = false;
                    self.seek(self.position + 1);
                }
                screen::divider(ui);
                let round = self.round_of(self.position);
                let turn = self.current.as_ref().map(|s| s.turn);
                let (rounds, unverified) = (self.rounds, self.unverified);
                let engine_warning = self.engine_warning.clone();
                let debug = self.debug;
                let mut go_to = None;
                let seed = match &self.source {
                    Source::Recording { seed, .. } => Some(seed.to_string()),
                    Source::Track(_) => self.battle.as_ref().map(|(b, _)| b.seed.clone()),
                };
                let battle = self.battle.as_ref().map(|(b, _)| b);
                // A track of a rumble battle is one of its rounds.
                let first = battle.map_or(0, |b| b.round.max(0) as usize);
                let rounds = battle.map_or(rounds, |b| b.rounds);
                screen::speed_and_readouts(
                    ui,
                    &mut self.turns_per_second,
                    seed.as_deref(),
                    |ui| {
                        if let Some(battle) = battle {
                            run_button(ui, battle, run);
                        }
                    },
                    |ui| {
                        if let Some(turn) = turn {
                            screen::info(ui, "Turn", turn.to_string());
                            ui.add_space(10.0);
                        }
                        egui::ComboBox::from_id_salt("round")
                            .selected_text(
                                RichText::new(format!("{} / {}", first + round + 1, rounds))
                                    .strong(),
                            )
                            .width(70.0)
                            .show_ui(ui, |ui| {
                                for (r, &start) in self.round_starts.iter().enumerate() {
                                    if ui
                                        .selectable_label(
                                            r == round,
                                            format!("Round {}", first + r + 1),
                                        )
                                        .clicked()
                                    {
                                        go_to = Some(start);
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Go to a round");
                        ui.label(RichText::new("Round").weak());
                        if unverified > 0 {
                            ui.add_space(10.0);
                            ui.colored_label(
                                Color32::LIGHT_RED,
                                format!("{unverified} turns differ from the recording"),
                            );
                        }
                        if debug {
                            ui.add_space(10.0);
                            ui.colored_label(Color32::GOLD, "Debug battle")
                                .on_hover_text(
                                    "A robot from an IDE was in it, not timed: not comparable \
                                 with real battles.",
                                );
                        }
                        if let Some(warning) = &engine_warning {
                            ui.add_space(10.0);
                            ui.colored_label(Color32::GOLD, "Other engine version")
                                .on_hover_text(warning);
                        }
                    },
                    |ui| {
                        ui.add_space(10.0);
                        ui.add(egui::Label::new(RichText::new(&self.title).weak()).truncate());
                    },
                );
                if let Some(start) = go_to {
                    self.seek(start);
                }
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(root, |ui| {
                let mut shown = self.current.clone();
                if let Some(s) = &mut shown {
                    for (r, visible) in s.robots.iter_mut().zip(&self.paint_visible) {
                        if !visible {
                            r.paint.clear();
                        }
                    }
                }
                let field = Field {
                    width: self.width,
                    height: self.height,
                    robots: &self.robots,
                    sentry_border: self.sentry_border,
                };
                let turns = (self.previous.as_ref(), shown.as_ref());
                let available = ui.max_rect();
                let size = (self.width, self.height);
                let navigation = view::Navigation::Free;
                view::navigate(ui, &mut self.camera, size, available, navigation);
                view::draw(ui, sprites, &field, style, turns, available, &self.camera);
            });
        // A track has no scores, so no results.
        let at_end = full && self.turns > 0 && self.position + 1 >= self.turns;
        if !at_end {
            self.results_closed = false;
        } else if !self.results_closed {
            let shown = screen::Outcome {
                title: "Results",
                robots: &self.robots,
                results: &self.results,
                snapshot: self.current.as_ref(),
                stopped: false,
            };
            let id = egui::Id::new(("replay-results", &self.title));
            let mut again = false;
            let request = screen::results_window(ctx, sprites, id, &shown, |ui| {
                if ui.button("Close").clicked() {
                    self.results_closed = true;
                }
                let button = egui::Button::new(RichText::new("Watch Again").strong());
                if ui
                    .add(button)
                    .on_hover_text("The replay from its first turn.")
                    .clicked()
                {
                    again = true;
                }
            });
            if request.is_some() {
                *save = request;
            }
            if again {
                self.seek(0);
                self.playing = true;
            }
        }
        back
    }
}

/// "Run this battle", for a track's rumble battle: sets `run` when clicked.
fn run_button(ui: &mut egui::Ui, battle: &TrackBattle, run: &mut bool) {
    let why = crate::rerun::unavailable(battle);
    let button = ui.add_enabled(why.is_none(), egui::Button::new("Run this battle"));
    let button = match why {
        Some(why) => button.on_disabled_hover_text(why),
        None => button.on_hover_text(format!(
            "This is round {} of a rumble battle of {}. Run the whole battle here, as the \
             rumble did, for the scores, console output and debug properties: it plays \
             fast up to round {} and pauses at its start. Robots that aren't here yet \
             come from the rumble.",
            battle.round + 1,
            battle.rounds,
            battle.round + 1
        )),
    };
    if button.clicked() {
        *run = true;
    }
}

/// A replay's title: its file name.
fn title(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use robocode_s_server::recording::{FORMAT, Recorder};

    /// A recording of a battle that never started: just its header.
    fn header_only(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{name}-{}.rsr", std::process::id()));
        let header = v1::RecordingHeader {
            format: FORMAT,
            ..Default::default()
        };
        Recorder::create(&path, header).unwrap().finish().unwrap();
        path
    }

    #[test]
    fn a_cancelled_opening_stops() {
        let path = header_only("robocodes-cancelled-replay");
        let opened = ReplayView::open(&path, &|_| {}, &AtomicBool::new(true));
        assert_eq!(opened.err().as_deref(), Some("cancelled"));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn opening_runs_in_the_background_and_reports_errors() {
        let ctx = egui::Context::default();
        let missing = std::env::temp_dir().join("robocodes-no-such-replay.rsr");
        let opening = Opening::start(&ctx, missing);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        let result = loop {
            if let Some(result) = opening.poll() {
                break result;
            }
            assert!(Instant::now() < deadline, "the opening ends");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(result.is_err());
    }
}
