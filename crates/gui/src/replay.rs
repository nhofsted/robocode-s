// SPDX-License-Identifier: EPL-2.0

//! Playing a recording back (design §11): the engine re-runs the recorded inputs.
//! Opening a recording runs it once and keeps a keyframe (a clone of the replay) every
//! [`KEYFRAME`] turns, so any turn can be shown by replaying at most that many.

use crate::sprites::Sprites;
use crate::view::{self, Field, Style};
use eframe::egui::{self, Color32, RichText};
use robocode_s_protocol::v1;
use robocode_s_server::recording::{Frame, Recording, Replay, round_result};
use robocode_s_server::results::contestants;
use std::path::Path;
use std::time::Instant;

const KEYFRAME: usize = 200;

pub struct ReplayView {
    title: String,
    robots: Vec<v1::RobotStatics>,
    width: f64,
    height: f64,
    rounds: i32,
    seed: i64,
    /// Replays positioned so that their next snapshot is turn `KEYFRAME * k`.
    keyframes: Vec<Replay>,
    /// Index of the first turn of each round.
    round_starts: Vec<usize>,
    /// Console output: (turn index, robot, text).
    output: Vec<(usize, usize, String)>,
    turns: usize,
    unverified: usize,
    results: Vec<v1::Results>,
    /// The shown turn and the replay positioned after it.
    position: usize,
    cursor: Replay,
    current: Option<v1::TurnSnapshot>,
    /// The turn before the shown one, for the scan arcs.
    previous: Option<v1::TurnSnapshot>,
    playing: bool,
    turns_per_second: f64,
    last_tick: Option<Instant>,
    paint_visible: Vec<bool>,
}

/// The next snapshot of a replay, skipping round ends.
fn next_snapshot(replay: &mut Replay) -> Option<(v1::TurnSnapshot, bool)> {
    replay.find_map(|f| match f {
        Frame::Snapshot { snapshot, verified } => Some((snapshot, verified)),
        Frame::RoundOver(_) => None,
    })
}

impl ReplayView {
    pub fn open(path: &Path) -> Result<Self, String> {
        let recording = Recording::read(path).map_err(|e| e.to_string())?;
        let mut replay = recording.replay();
        let (mut keyframes, mut round_starts, mut output) = (Vec::new(), Vec::new(), Vec::new());
        let (mut turns, mut unverified, mut round) = (0usize, 0usize, -1);
        loop {
            if turns % KEYFRAME == 0 {
                keyframes.push(replay.clone());
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
            }
            unverified += usize::from(!verified);
            turns += 1;
        }
        let results = round_result(replay.battle()).results;
        let rules = recording.header.rules.unwrap_or_default();
        let robots = recording.header.robots.clone();
        let mut view = Self {
            title: path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            paint_visible: vec![true; robots.len()],
            robots,
            width: f64::from(rules.battlefield_width),
            height: f64::from(rules.battlefield_height),
            rounds: rules.num_rounds,
            seed: recording.header.seed,
            cursor: keyframes[0].clone(),
            keyframes,
            round_starts,
            output,
            turns,
            unverified,
            results,
            position: 0,
            current: None,
            previous: None,
            playing: true,
            turns_per_second: 30.0,
            last_tick: None,
        };
        view.seek(0);
        Ok(view)
    }

    /// Shows turn `index` (clamped).
    fn seek(&mut self, index: usize) {
        if self.turns == 0 {
            return;
        }
        let index = index.min(self.turns - 1);
        if self.current.is_some() && index == self.position + 1 {
            self.previous = self.current.take();
        } else {
            // From the keyframe before the turn before, so that turn is known as well.
            let before = index.saturating_sub(1);
            self.cursor = self.keyframes[before / KEYFRAME].clone();
            for _ in 0..before % KEYFRAME {
                next_snapshot(&mut self.cursor);
            }
            self.previous = if index > 0 {
                next_snapshot(&mut self.cursor).map(|(s, _)| s)
            } else {
                None
            };
        }
        self.current = next_snapshot(&mut self.cursor).map(|(s, _)| s);
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
        let due = (now - last).as_secs_f64() * self.turns_per_second;
        if due >= 1.0 {
            let steps = (due as usize).min(1000);
            let target = (self.position + steps).min(self.turns - 1);
            if target == self.position + 1 {
                self.seek(target);
            } else {
                // Larger jumps: replay forward from here.
                while self.position < target {
                    let next = self.position + 1;
                    self.seek(next);
                }
            }
            self.last_tick = Some(
                last + std::time::Duration::from_secs_f64(steps as f64 / self.turns_per_second),
            );
        }
        ctx.request_repaint();
    }

    /// Draws the replay; true when the user closes it.
    pub fn ui(
        &mut self,
        root: &mut egui::Ui,
        sprites: &mut Sprites,
        style: Style,
        selected: &mut usize,
    ) -> bool {
        let ctx = &root.ctx().clone();
        self.play(ctx);
        let mut back = false;
        egui::Panel::top("replay-controls").show(root, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("Close")
                    .on_hover_text("open another recording")
                    .clicked()
                {
                    back = true;
                }
                ui.separator();
                let label = if self.playing { "Pause" } else { "Play" };
                if ui.button(label).clicked() {
                    self.playing = !self.playing;
                    self.last_tick = None;
                }
                if ui.button("⏴").on_hover_text("previous turn").clicked() {
                    self.playing = false;
                    self.seek(self.position.saturating_sub(1));
                }
                if ui.button("⏵").on_hover_text("next turn").clicked() {
                    self.playing = false;
                    self.seek(self.position + 1);
                }
                ui.label("Turns/s");
                ui.add(
                    egui::Slider::new(&mut self.turns_per_second, 1.0..=2000.0).logarithmic(true),
                );
                ui.separator();
                let round = self.round_of(self.position);
                egui::ComboBox::from_id_salt("round")
                    .selected_text(format!("Round {}/{}", round + 1, self.rounds))
                    .show_ui(ui, |ui| {
                        for (r, &start) in self.round_starts.clone().iter().enumerate() {
                            if ui
                                .selectable_label(r == round, format!("Round {}", r + 1))
                                .clicked()
                            {
                                self.seek(start);
                            }
                        }
                    });
                if let Some(s) = &self.current {
                    ui.label(format!("turn {}", s.turn));
                }
                ui.label(RichText::new(format!("{} · seed {}", self.title, self.seed)).weak());
                if self.unverified > 0 {
                    ui.colored_label(
                        Color32::LIGHT_RED,
                        format!("{} turns differ from the recording", self.unverified),
                    );
                }
            });
            let mut position = self.position;
            ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(0.0);
            let slider = ui.add(
                egui::Slider::new(&mut position, 0..=self.turns.saturating_sub(1))
                    .show_value(false)
                    .trailing_fill(true),
            );
            if slider.changed() {
                self.seek(position);
            }
        });
        egui::Panel::right("replay-robots")
            .default_size(260.0)
            .show(root, |ui| {
                ui.label(RichText::new("Robots").strong());
                for (i, statics) in self.robots.iter().enumerate() {
                    let robot = self.current.as_ref().and_then(|s| s.robots.get(i));
                    let energy = robot.and_then(|r| r.energy).unwrap_or_default();
                    if ui.selectable_label(*selected == i, &statics.name).clicked() {
                        *selected = i;
                    }
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::ProgressBar::new((energy / 100.0).clamp(0.0, 1.0) as f32)
                                .desired_width(120.0)
                                .text(format!("{energy:.1}")),
                        );
                        ui.checkbox(&mut self.paint_visible[i], "paint");
                    });
                }
            });
        egui::Panel::bottom("replay-console")
            .resizable(true)
            .default_size(140.0)
            .show(root, |ui| {
                let i = (*selected).min(self.robots.len().saturating_sub(1));
                let text: String = self
                    .output
                    .iter()
                    .take_while(|(turn, _, _)| *turn <= self.position)
                    .filter(|(_, robot, _)| *robot == i)
                    .map(|(_, _, t)| t.as_str())
                    .collect();
                ui.label(
                    RichText::new(self.robots.get(i).map_or("", |r| r.name.as_str())).strong(),
                );
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| ui.label(RichText::new(text).monospace()));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
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
                };
                let turns = (self.previous.as_ref(), shown.as_ref());
                view::draw(ui, sprites, &field, style, turns, ui.max_rect());
            });
        if self.position + 1 >= self.turns {
            egui::Window::new("Results")
                .collapsible(false)
                .show(ctx, |ui| {
                    egui::Grid::new("replay-results")
                        .striped(true)
                        .show(ui, |ui| {
                            for c in contestants(&self.robots, &self.results) {
                                ui.label(c.rank.to_string());
                                ui.label(&c.name);
                                ui.label(format!("{:.0}", c.score));
                                ui.end_row();
                            }
                        });
                });
        }
        back
    }
}
