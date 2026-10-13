// SPDX-License-Identifier: EPL-2.0

//! The application: battle setup, a live battle, or a replay.

use crate::live::Live;
use crate::replay::ReplayView;
use crate::sprites::Sprites;
use crate::view::{self, Field};
use eframe::egui::{self, Color32, RichText};
use robocode_s_engine::BattleRules;
use robocode_s_protocol::v1;
use robocode_s_runner::local::{Environment, prepare, random_seed};
use robocode_s_server::ServerConfig;
use robocode_s_server::results::contestants;
use std::path::PathBuf;

/// The battle setup form.
#[derive(Clone, Debug)]
pub struct Setup {
    pub robots: String,
    pub rounds: i32,
    pub seed: String,
    pub width: i32,
    pub height: i32,
    pub classic: bool,
    pub record: String,
    pub paint: bool,
    pub turns_per_second: u32,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            robots: "sample.Fire, sample.Walls, sample.Crazy, sample.SpinBot".into(),
            rounds: 10,
            seed: String::new(),
            width: 800,
            height: 600,
            classic: false,
            record: String::new(),
            paint: false,
            turns_per_second: 30,
        }
    }
}

/// Saves a screenshot to a file once the condition holds, then quits (for checks).
pub struct Screenshot {
    pub path: PathBuf,
    /// Live: wait for this turn of the first round; otherwise this many frames.
    pub after: i64,
    requested: bool,
    frames: i64,
}

impl Screenshot {
    pub fn new(path: PathBuf, after: i64) -> Self {
        Self {
            path,
            after,
            requested: false,
            frames: 0,
        }
    }
}

enum Screen {
    Setup,
    Live(Box<Live>),
    Replay(Box<ReplayView>),
}

pub struct App {
    sprites: Sprites,
    env: Environment,
    setup: Setup,
    screen: Screen,
    selected: usize,
    error: Option<String>,
    screenshot: Option<Screenshot>,
    open_path: String,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext,
        env: Environment,
        setup: Setup,
        start: bool,
        replay: Option<PathBuf>,
        screenshot: Option<Screenshot>,
    ) -> Self {
        let mut app = Self {
            sprites: Sprites::new(&cc.egui_ctx),
            env,
            setup,
            screen: Screen::Setup,
            selected: 0,
            error: None,
            screenshot,
            open_path: String::new(),
        };
        if let Some(path) = replay {
            app.open_replay(path);
        } else if start {
            app.start(&cc.egui_ctx);
        }
        app
    }

    fn start(&mut self, ctx: &egui::Context) {
        let s = &self.setup;
        let robots: Vec<String> = s
            .robots
            .split(',')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(String::from)
            .collect();
        let seed = match s.seed.trim() {
            "" => random_seed(),
            text => match text.parse() {
                Ok(seed) => seed,
                Err(_) => {
                    self.error = Some(format!("not a number: {text}"));
                    return;
                }
            },
        };
        let config = ServerConfig {
            rules: BattleRules {
                width: s.width,
                height: s.height,
                num_rounds: s.rounds.max(1),
                ..Default::default()
            },
            seed,
            deterministic: !s.classic,
            record: (!s.record.trim().is_empty()).then(|| PathBuf::from(s.record.trim())),
            ..Default::default()
        };
        match prepare(&self.env, &robots, config, 512) {
            Ok((host, battle)) => {
                let mut live = Live::start(ctx, host, battle);
                live.seed = seed;
                live.set_speed(s.turns_per_second);
                if s.paint {
                    for i in 0..live.robots.len() {
                        live.set_paint(i, true);
                    }
                }
                self.selected = 0;
                self.error = None;
                self.screen = Screen::Live(Box::new(live));
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn open_replay(&mut self, path: PathBuf) {
        match ReplayView::open(&path) {
            Ok(view) => {
                self.selected = 0;
                self.error = None;
                self.screen = Screen::Replay(Box::new(view));
            }
            Err(e) => self.error = Some(format!("{}: {e}", path.display())),
        }
    }

    fn setup_ui(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        egui::CentralPanel::default().show(root, |ui| {
            ui.heading("robocode-s");
            ui.add_space(8.0);
            let s = &mut self.setup;
            egui::Grid::new("setup")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Robots");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.robots)
                            .desired_width(480.0)
                            .hint_text("sample.Fire, sample.Walls, sampleteam.MyFirstTeam"),
                    );
                    ui.end_row();
                    ui.label("Rounds");
                    ui.add(egui::DragValue::new(&mut s.rounds).range(1..=10_000));
                    ui.end_row();
                    ui.label("Battlefield");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.width).range(400..=5000));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut s.height).range(400..=5000));
                    });
                    ui.end_row();
                    ui.label("Seed");
                    ui.add(egui::TextEdit::singleline(&mut s.seed).hint_text("random"));
                    ui.end_row();
                    ui.label("Mode");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.classic, false, "Deterministic");
                        ui.radio_value(&mut s.classic, true, "Classic timing");
                    });
                    ui.end_row();
                    ui.label("Record to");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.record)
                            .hint_text("file (optional)")
                            .desired_width(480.0),
                    );
                    ui.end_row();
                    ui.label("Debug paint");
                    ui.checkbox(&mut s.paint, "for every robot");
                    ui.end_row();
                });
            ui.add_space(12.0);
            if ui.button(RichText::new("Start battle").strong()).clicked() {
                self.start(ctx);
            }
            ui.add_space(24.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Replay a recording");
                ui.add(egui::TextEdit::singleline(&mut self.open_path).desired_width(400.0));
                if ui.button("Open").clicked() && !self.open_path.trim().is_empty() {
                    let path = PathBuf::from(self.open_path.trim());
                    self.open_replay(path);
                }
            });
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!(
                    "Robots are looked up in {}.",
                    self.env
                        .jars
                        .iter()
                        .map(|j| j.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .weak(),
            );
            if let Some(e) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
        });
    }
}

/// Debug paint toggles: which robots paint, and what to call on a change.
type PaintToggles<'a> = Option<(&'a [bool], &'a mut dyn FnMut(usize, bool))>;

/// The robot list: energy, score, debug paint toggles; a click selects a robot's console.
fn robot_list(
    ui: &mut egui::Ui,
    robots: &[v1::RobotStatics],
    snapshot: Option<&v1::TurnSnapshot>,
    selected: &mut usize,
    mut paint: PaintToggles,
) {
    for (i, statics) in robots.iter().enumerate() {
        let robot = snapshot.and_then(|s| s.robots.get(i));
        let energy = robot.and_then(|r| r.energy).unwrap_or_default();
        let dead = robot.is_some_and(|r| r.state == v1::RobotState::Dead as i32);
        ui.horizontal(|ui| {
            let name = RichText::new(&statics.name);
            let name = if dead { name.weak() } else { name };
            if ui.selectable_label(*selected == i, name).clicked() {
                *selected = i;
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::ProgressBar::new((energy / 100.0).clamp(0.0, 1.0) as f32)
                    .desired_width(120.0)
                    .text(format!("{energy:.1}")),
            );
            ui.label(format!(
                "{:.0}",
                robot.map_or(0.0, |r| r.total_score.unwrap_or_default()
                    + r.score.unwrap_or_default())
            ));
            if let Some((enabled, toggle)) = paint.as_mut() {
                let mut on = enabled[i];
                if ui.checkbox(&mut on, "paint").changed() {
                    toggle(i, on);
                }
            }
        });
        ui.add_space(4.0);
    }
}

fn console(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.label(RichText::new(title).strong());
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
        });
}

fn results_window(ctx: &egui::Context, robots: &[v1::RobotStatics], results: &[v1::Results]) {
    egui::Window::new("Results")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let rows = contestants(robots, results);
            let total: f64 = rows.iter().map(|c| c.score).sum();
            egui::Grid::new("results").striped(true).show(ui, |ui| {
                for h in [
                    "Rank",
                    "Robot",
                    "Score",
                    "Survival",
                    "Bullet dmg",
                    "Ram dmg",
                    "1sts",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for c in &rows {
                    ui.label(c.rank.to_string());
                    ui.label(&c.name);
                    let pct = if total > 0.0 {
                        100.0 * c.score / total
                    } else {
                        0.0
                    };
                    ui.label(format!("{:.0} ({pct:.0}%)", c.score));
                    ui.label(format!("{:.0}", c.survival));
                    ui.label(format!("{:.0}", c.bullet_damage));
                    ui.label(format!("{:.0}", c.ram_damage));
                    ui.label(c.firsts.to_string());
                    ui.end_row();
                }
            });
        });
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let mut back = false;
        match &mut self.screen {
            Screen::Setup => self.setup_ui(root),
            Screen::Live(live) => {
                live.poll();
                egui::Panel::top("controls").show(root, |ui| {
                    ui.horizontal(|ui| {
                        let label = if live.paused { "Resume" } else { "Pause" };
                        if ui.button(label).clicked() {
                            let paused = !live.paused;
                            live.set_paused(paused);
                        }
                        if ui
                            .add_enabled(live.paused, egui::Button::new("Step"))
                            .clicked()
                        {
                            live.step();
                        }
                        let mut tps = live.turns_per_second;
                        let max = tps == 0;
                        ui.label("Turns/s");
                        let slider = ui.add_enabled(
                            !max,
                            egui::Slider::new(&mut tps, 1..=1000).logarithmic(true),
                        );
                        let mut fastest = max;
                        let toggled = ui.checkbox(&mut fastest, "max").changed();
                        if slider.changed() || toggled {
                            live.set_speed(if fastest { 0 } else { tps.max(1) });
                        }
                        ui.separator();
                        if let Some(s) = &live.latest {
                            ui.label(format!(
                                "Round {}/{} · turn {}",
                                s.round + 1,
                                live.rounds,
                                s.turn
                            ));
                        }
                        ui.label(RichText::new(format!("seed {}", live.seed)).weak());
                        if live.outcome.is_some() && ui.button("New battle").clicked() {
                            back = true;
                        }
                    });
                });
                egui::Panel::right("robots")
                    .default_size(260.0)
                    .show(root, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let paint = live.paint.clone();
                            let (robots, latest) = (live.robots.clone(), live.latest.clone());
                            robot_list(
                                ui,
                                &robots,
                                latest.as_ref(),
                                &mut self.selected,
                                Some((&paint, &mut |i, on| live.set_paint(i, on))),
                            );
                        });
                    });
                egui::Panel::bottom("console")
                    .resizable(true)
                    .default_size(140.0)
                    .show(root, |ui| {
                        let i = self.selected.min(live.robots.len().saturating_sub(1));
                        let title = live.robots.get(i).map_or("", |r| r.name.as_str());
                        console(ui, title, live.output.get(i).map_or("", String::as_str));
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| {
                        let field = Field {
                            width: live.width,
                            height: live.height,
                            robots: &live.robots,
                        };
                        view::draw(
                            ui,
                            &mut self.sprites,
                            &field,
                            live.latest.as_ref(),
                            ui.max_rect(),
                        );
                    });
                match &live.outcome {
                    Some(Ok(results)) => results_window(ctx, &live.robots, results),
                    Some(Err(e)) => {
                        egui::Window::new("Battle failed").show(ctx, |ui| {
                            ui.colored_label(Color32::LIGHT_RED, e);
                            if ui.button("Back").clicked() {
                                back = true;
                            }
                        });
                    }
                    None => {}
                }
            }
            Screen::Replay(replay) => {
                if replay.ui(root, &mut self.sprites, &mut self.selected) {
                    back = true;
                }
            }
        }
        if back {
            self.screen = Screen::Setup;
        }
        self.screenshot_step(ctx);
    }
}

impl App {
    /// `--screenshot`: once the battle is far enough (or after some frames), saves the
    /// window to a PNG and quits.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        if shot.requested {
            let image = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Err(e) = image::save_buffer(
                    &shot.path,
                    &rgba,
                    w as u32,
                    h as u32,
                    image::ExtendedColorType::Rgba8,
                ) {
                    eprintln!("screenshot: {e}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
            return;
        }
        shot.frames += 1;
        let ready = match &self.screen {
            Screen::Live(live) => {
                live.outcome.is_some()
                    || live
                        .latest
                        .as_ref()
                        .is_some_and(|s| s.turn >= shot.after || s.round > 0)
            }
            _ => shot.frames >= shot.after,
        };
        if ready {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
    }
}
