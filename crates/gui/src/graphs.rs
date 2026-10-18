// SPDX-License-Identifier: EPL-2.0

//! Graphs per robot below the robot list (#33): energy over this round or the whole
//! battle, and the CPU time each robot used per turn against its budget. Drawn with the
//! painter: compact, one line per robot in its body colour, or in its gun or radar
//! colour, or else a colour of its own, when another robot's line already looks like it.

use crate::shell;
use crate::sprites::{Rgb, default_colors, hsl_to_rgb, rgb_to_hsl};
use crate::view;
use eframe::egui::{self, Color32, FontId, RichText, Sense, Shape, Stroke, pos2, vec2};
use robocode_s_protocol::v1;

/// What the battle screen keeps of each turn for the graphs and the scores.
#[derive(Default)]
pub struct History {
    robots: usize,
    /// Per turn: its round.
    rounds: Vec<i32>,
    /// Per turn, per robot: energy.
    energy: Vec<f32>,
    /// Per turn, per robot: CPU time (ms).
    cpu: Vec<f32>,
    /// The CPU budget per turn (ms); 0 without one.
    budget: f32,
    /// Per round: each robot's total score at its first turn.
    round_totals: Vec<Vec<f64>>,
}

impl History {
    pub fn new(robots: usize) -> Self {
        Self {
            robots,
            ..Default::default()
        }
    }

    /// Turns kept.
    pub fn len(&self) -> usize {
        self.rounds.len()
    }

    pub fn push(&mut self, snapshot: &v1::TurnSnapshot) {
        if self.rounds.last() != Some(&snapshot.round) {
            let round = usize::try_from(snapshot.round).unwrap_or_default();
            if self.round_totals.len() <= round {
                self.round_totals.resize(round + 1, Vec::new());
            }
            self.round_totals[round] = snapshot
                .robots
                .iter()
                .map(|r| r.total_score.unwrap_or_default())
                .collect();
        }
        self.rounds.push(snapshot.round);
        for i in 0..self.robots {
            let r = snapshot.robots.get(i);
            self.energy
                .push(r.and_then(|r| r.energy).unwrap_or_default() as f32);
            self.cpu.push(r.map_or(0.0, |r| r.cpu_nanos as f32 / 1e6));
            if let Some(budget) = r.map(|r| r.budget_nanos).filter(|&b| b > 0) {
                self.budget = budget as f32 / 1e6;
            }
        }
    }

    /// A robot's score in the battle so far: its total plus this round's score. The engine
    /// adds the round's score to the total when the round ends (classic's
    /// `generateTotals`), so a total that changed since the round's first turn already
    /// holds it.
    pub fn score(&self, round: i32, i: usize, robot: &v1::RobotSnapshot) -> f64 {
        let total = robot.total_score.unwrap_or_default();
        let at_start = usize::try_from(round)
            .ok()
            .and_then(|r| self.round_totals.get(r))
            .and_then(|t| t.get(i));
        match at_start {
            Some(&start) if start != total => total,
            _ => total + robot.score.unwrap_or_default(),
        }
    }

    /// The turns a graph covers when `shown` turns are shown: this round's, or all.
    fn range(&self, shown: usize, battle: bool) -> std::ops::Range<usize> {
        let end = shown.min(self.len());
        if battle || end == 0 {
            return 0..end;
        }
        let round = self.rounds[end - 1];
        let start = self.rounds[..end].partition_point(|&r| r < round);
        start..end
    }
}

/// Where the graphs' Round/Battle choice is kept: one for battles and replays alike.
fn whole_battle_id() -> egui::Id {
    egui::Id::new("graphs-battle")
}

/// The graphs show the whole battle rather than the round (the option's choice).
pub fn whole_battle(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(whole_battle_id()))
        .unwrap_or(false)
}

pub fn set_whole_battle(ctx: &egui::Context, battle: bool) {
    ctx.data_mut(|d| d.insert_temp(whole_battle_id(), battle));
}

/// The graphs, `height` tall, below the robot list: energy and CPU for the first `shown`
/// turns of `history`. Without `cpu` (a viewer track: one round, no CPU times), energy only.
pub fn graphs(
    ui: &mut egui::Ui,
    robots: &[v1::RobotStatics],
    snapshot: Option<&v1::TurnSnapshot>,
    history: &History,
    shown: usize,
    height: f32,
    cpu: bool,
) {
    let tanks: Vec<[Rgb; 3]> = robots
        .iter()
        .enumerate()
        .map(
            |(i, statics)| match snapshot.and_then(|s| s.robots.get(i)) {
                Some(robot) => view::colors(statics, robot).0,
                None => default_colors(&statics.full_class_name),
            },
        )
        .collect();
    let colors = line_colors(&tanks);
    let mut battle = whole_battle(ui.ctx());
    // Each graph: a heading line and the plot.
    let s = ui.spacing().item_spacing.y;
    let plot = if cpu {
        ((height - 2.0 * (18.0 + 4.0 + 3.0 * s) - 8.0) / 2.0).max(40.0)
    } else {
        (height - (18.0 + 4.0 + 3.0 * s)).max(40.0)
    };

    let range = history.range(shown, battle);
    heading(ui, "ENERGY", |ui| {
        if !cpu {
            return;
        }
        for (label, value) in [("Battle", true), ("Round", false)] {
            let text = RichText::new(label).size(11.0);
            if ui
                .selectable_label(battle == value, text)
                .on_hover_text(if value {
                    "The whole battle"
                } else {
                    "This round"
                })
                .clicked()
            {
                battle = value;
            }
        }
    });
    set_whole_battle(ui.ctx(), battle);
    let energy = Plot {
        history,
        values: &history.energy,
        range: range.clone(),
        top: None,
        limit: None,
        smooth: 1,
        bucket: Bucket::Last,
        unit: "",
    };
    energy.show(ui, robots, &colors, plot);
    if !cpu {
        return;
    }

    ui.add_space(8.0);
    let budget = history.budget;
    heading(ui, "CPU", |ui| {
        let text = if budget > 0.0 {
            format!("per turn, of {budget:.0} ms")
        } else {
            "per turn".to_string()
        };
        ui.label(RichText::new(text).size(11.0).weak())
            .on_hover_text(if budget > 0.0 {
                "CPU time each robot used per turn (averaged over a few turns), against \
                 its turn budget (red; along the top when it's above the scale)"
            } else {
                "Work each robot did per turn (averaged over a few turns), counted and \
                 converted to time as on the reference machine; deterministic mode has no \
                 time budget"
            });
    });
    let cpu = Plot {
        history,
        values: &history.cpu,
        range,
        // Scaled to the usual highest time, up to a bit above the budget.
        top: (budget > 0.0).then_some(budget * 1.25),
        limit: (budget > 0.0).then_some(budget),
        smooth: CPU_SMOOTHING,
        bucket: Bucket::Max,
        unit: " ms",
    };
    cpu.show(ui, robots, &colors, plot);
}

/// The CPU graph averages this many turns: per-turn times jitter a lot.
const CPU_SMOOTHING: usize = 8;

/// A graph's heading: its name on the left, `right` on the right.
fn heading(ui: &mut egui::Ui, name: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(RichText::new(name).size(12.0).strong().color(shell::ACCENT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                right(ui);
            });
        },
    );
    ui.add_space(4.0);
}

/// How the turns that share a pixel column become one value.
#[derive(Clone, Copy)]
enum Bucket {
    /// The last turn's (energy).
    Last,
    /// The highest (CPU, so spikes stay visible).
    Max,
}

struct Plot<'a> {
    history: &'a History,
    /// Per turn, per robot.
    values: &'a [f32],
    range: std::ops::Range<usize>,
    /// The scale comes from the values, up to this (higher values are clipped).
    top: Option<f32>,
    /// The CPU budget: a red line at this value, or along the top when it's off the scale.
    limit: Option<f32>,
    /// Each point is the mean of this many turns, up to it.
    smooth: usize,
    bucket: Bucket,
    unit: &'a str,
}

impl Plot<'_> {
    /// Per robot, its values over the range, averaged over `smooth` turns.
    fn series(&self) -> Vec<Vec<f32>> {
        let n = self.history.robots;
        (0..n)
            .map(|i| {
                let value = |k: usize| self.values.get(k * n + i).copied().unwrap_or_default();
                let mut sum = 0.0;
                self.range
                    .clone()
                    .map(|k| {
                        sum += value(k);
                        let first = self.range.start.max((k + 1).saturating_sub(self.smooth));
                        if k >= self.range.start + self.smooth {
                            sum -= value(k - self.smooth);
                        }
                        sum / (k + 1 - first) as f32
                    })
                    .collect()
            })
            .collect()
    }

    fn show(
        &self,
        ui: &mut egui::Ui,
        robots: &[v1::RobotStatics],
        colors: &[Color32],
        height: f32,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect.expand(1.0));
        painter.rect_filled(rect, 0.0, shell::BAR);
        let series = self.series();
        let top = typical_max(&series, self.unit.is_empty());
        let top = self.top.map_or(top, |most| top.min(most));
        let turns = self.range.len();
        // At least 100 turns wide, so a round's first turns don't stretch across.
        let span = turns.max(100) as f32;
        let x = |k: f32| rect.left() + rect.width() * k / span;
        let y = |v: f32| rect.bottom() - rect.height() * (v / top).clamp(0.0, 1.0);
        let grid = Stroke::new(1.0, shell::LINE.gamma_multiply(0.6));
        for f in [0.25, 0.5, 0.75] {
            painter.hline(rect.x_range(), rect.bottom() - rect.height() * f, grid);
        }
        // Round starts, in the whole battle.
        let rounds = &self.history.rounds;
        for k in self.range.clone().skip(1) {
            if rounds[k] != rounds[k - 1] {
                let at = x((k - self.range.start) as f32);
                painter.vline(at, rect.y_range(), grid);
            }
        }
        let columns = rect.width().max(1.0) as usize;
        for (values, &color) in series.iter().zip(colors) {
            let mut points = Vec::new();
            if turns <= columns {
                for (t, &v) in values.iter().enumerate() {
                    points.push(pos2(x(t as f32), y(v)));
                }
            } else {
                // A point per pixel column.
                let per = turns as f32 / columns as f32;
                for c in 0..columns {
                    let from = (c as f32 * per) as usize;
                    let to = (((c + 1) as f32 * per) as usize).clamp(from + 1, turns);
                    let v = match self.bucket {
                        Bucket::Last => values[to - 1],
                        Bucket::Max => values[from..to].iter().copied().fold(0.0, f32::max),
                    };
                    points.push(pos2(rect.left() + c as f32 + 0.5, y(v)));
                }
            }
            if points.len() > 1 {
                painter.add(Shape::line(points, Stroke::new(1.2, color)));
            }
        }
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, shell::LINE),
            egui::StrokeKind::Inside,
        );
        // The budget over the robots' lines: dashed at its value, or solid along the top
        // when the scale stays below it, labelled top right.
        if let Some(limit) = self.limit {
            let red = Color32::from_rgb(0xd0, 0x40, 0x40);
            let stroke = Stroke::new(1.5, red);
            if limit <= top {
                let at = y(limit);
                painter.extend(Shape::dashed_line(
                    &[pos2(rect.left(), at), pos2(rect.right(), at)],
                    stroke,
                    4.0,
                    3.0,
                ));
            } else {
                painter.hline(rect.x_range(), rect.top() + 0.75, stroke);
            }
            let label = painter.layout_no_wrap(
                format!("budget {}{}", trimmed(limit), self.unit),
                FontId::proportional(10.0),
                red,
            );
            let at = pos2(
                rect.right() - label.size().x - 4.0,
                (y(limit) + 2.0).min(rect.bottom() - label.size().y - 2.0),
            );
            painter.rect_filled(
                egui::Rect::from_min_size(at - vec2(2.0, 0.0), label.size() + vec2(4.0, 0.0)),
                2.0,
                shell::BAR.gamma_multiply(0.85),
            );
            painter.galley(at, label, red);
        }
        // The scale, top left, on a backing so lines don't run through it.
        let scale = painter.layout_no_wrap(
            format!("{}{}", trimmed(top), self.unit),
            FontId::proportional(10.0),
            Color32::from_gray(130),
        );
        let at = rect.left_top() + vec2(2.0, 2.0);
        painter.rect_filled(
            egui::Rect::from_min_size(at, scale.size() + vec2(4.0, 0.0)),
            2.0,
            shell::BAR.gamma_multiply(0.85),
        );
        painter.galley(at + vec2(2.0, 0.0), scale, Color32::from_gray(130));
        // Hovered: the values at that turn.
        if let Some(pos) = response.hover_pos()
            && turns > 0
        {
            let t = (((pos.x - rect.left()) / rect.width() * span) as usize).min(turns - 1);
            painter.vline(
                x(t as f32),
                rect.y_range(),
                Stroke::new(1.0, Color32::from_gray(90)),
            );
            response.on_hover_ui_at_pointer(|ui| {
                for ((statics, values), &color) in robots.iter().zip(&series).zip(colors) {
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(dot, 0.0, color);
                        ui.label(format!(
                            "{}{}  {}",
                            trimmed(values[t]),
                            self.unit,
                            statics.name
                        ));
                    });
                }
            });
        }
    }
}

/// A scale from the values: for energy at least 100 and the highest; for CPU times the
/// usual highest, so a robot's first turn (class loading) or a garbage collection doesn't
/// flatten the rest.
fn typical_max(series: &[Vec<f32>], energy: bool) -> f32 {
    let mut all: Vec<f32> = series.iter().flatten().copied().collect();
    if energy {
        return all.into_iter().fold(100.0, f32::max) * 1.05;
    }
    if all.is_empty() {
        return 1.0;
    }
    // The 98th percentile, without sorting all of a long battle's turns every frame.
    let k = (all.len() - 1) * 98 / 100;
    let (_, &mut p98, _) = all.select_nth_unstable_by(k, f32::total_cmp);
    (p98 * 1.25).max(0.1)
}

/// A number for a scale or tooltip: whole above 10, else with a decimal or two.
fn trimmed(v: f32) -> String {
    if v >= 10.0 {
        format!("{v:.0}")
    } else if v >= 1.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// Each robot's line colour: the first of its body, gun and radar colours that is far
/// enough from the lines before it, or else the hue furthest from all of them.
fn line_colors(tanks: &[[Rgb; 3]]) -> Vec<Color32> {
    let mut chosen: Vec<Rgb> = Vec::with_capacity(tanks.len());
    for tank in tanks {
        let nearest = |c: Rgb, chosen: &[Rgb]| {
            chosen
                .iter()
                .map(|&o| distance(c, o))
                .fold(f32::INFINITY, f32::min)
        };
        let own = tank
            .iter()
            .map(|&c| visible(c))
            .find(|&c| nearest(c, &chosen) >= DISTINCT);
        let color = own.unwrap_or_else(|| {
            (0..24)
                .map(|k| visible(hsl_to_rgb(k as f32 / 24.0, 0.8, 0.5)))
                .max_by(|&a, &b| nearest(a, &chosen).total_cmp(&nearest(b, &chosen)))
                .expect("24 hues")
        });
        chosen.push(color);
    }
    chosen
        .into_iter()
        .map(|[r, g, b]| Color32::from_rgb(r, g, b))
        .collect()
}

/// How far apart two line colours must be to tell them apart on the graphs.
const DISTINCT: f32 = 110.0;

/// The "redmean" approximation of how different two colours look.
fn distance(a: Rgb, b: Rgb) -> f32 {
    let [r1, g1, b1] = a.map(f32::from);
    let [r2, g2, b2] = b.map(f32::from);
    let mean = (r1 + r2) / 2.0;
    let (dr, dg, db) = (r1 - r2, g1 - g2, b1 - b2);
    ((2.0 + mean / 256.0) * dr * dr + 4.0 * dg * dg + (2.0 + (255.0 - mean) / 256.0) * db * db)
        .sqrt()
}

/// A robot's colour lightened enough to show on the dark graphs. Raising its lightness
/// rather than mixing in white keeps it vivid; only a garish saturation is toned down.
fn visible(color: Rgb) -> Rgb {
    const MIN: f32 = 110.0;
    let luma = |[r, g, b]: Rgb| 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    let [h, s, l] = rgb_to_hsl(color);
    let s = s.min(0.85);
    let mut l = l;
    let mut out = hsl_to_rgb(h, s, l);
    while luma(out) < MIN && l < 1.0 {
        l = (l + 0.02).min(1.0);
        out = hsl_to_rgb(h, s, l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_scale_ignores_rare_spikes() {
        let mut times: Vec<f32> = (0..1000).map(|k| (k % 10) as f32).collect();
        times[500] = 1000.0;
        let top = typical_max(&[times.clone(), times], false);
        assert_eq!(top, 9.0 * 1.25);
        assert_eq!(typical_max(&[], false), 1.0);
        assert_eq!(typical_max(&[vec![50.0, 120.0]], true), 120.0 * 1.05);
    }

    fn snapshot(round: i32, totals: &[f64], scores: &[f64]) -> v1::TurnSnapshot {
        v1::TurnSnapshot {
            round,
            robots: totals
                .iter()
                .zip(scores)
                .map(|(&t, &s)| v1::RobotSnapshot {
                    total_score: Some(t),
                    score: Some(s),
                    cpu_nanos: 2_000_000,
                    budget_nanos: 50_000_000,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_score_counts_the_round_once_after_its_totals() {
        let mut history = History::new(1);
        let start = snapshot(1, &[100.0], &[0.0]);
        history.push(&start);
        let during = snapshot(1, &[100.0], &[40.0]);
        history.push(&during);
        assert_eq!(history.score(1, 0, &during.robots[0]), 140.0);
        // The round ended: its score is in the total, and still in the round's score.
        let ended = snapshot(1, &[150.0], &[40.0]);
        history.push(&ended);
        assert_eq!(history.score(1, 0, &ended.robots[0]), 150.0);
        assert_eq!(history.budget, 50.0);
        assert_eq!(history.cpu[0], 2.0);
    }

    #[test]
    fn robots_of_one_colour_get_lines_apart() {
        let red = [[200, 30, 30], [120, 120, 120], [200, 30, 30]];
        let green = [[200, 30, 30], [30, 180, 30], [200, 30, 30]];
        let lines = line_colors(&[red, red, green, red]);
        // The second red robot falls back on its grey gun, the green-gunned one on its gun.
        assert_eq!(lines[0].to_array()[..3], visible(red[0]));
        assert_eq!(lines[1].to_array()[..3], visible([120, 120, 120]));
        assert_eq!(lines[2].to_array()[..3], visible([30, 180, 30]));
        let rgb = |c: Color32| -> Rgb { c.to_array()[..3].try_into().unwrap() };
        for (i, &a) in lines.iter().enumerate() {
            for &b in &lines[..i] {
                assert!(distance(rgb(a), rgb(b)) >= DISTINCT, "{a:?} near {b:?}");
            }
        }
    }

    #[test]
    fn a_dark_colour_lightens_without_greying() {
        let [r, g, b] = visible([120, 0, 0]);
        let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        assert!(luma >= 110.0);
        assert!(rgb_to_hsl([r, g, b])[1] >= 0.8, "{r} {g} {b}");
    }

    #[test]
    fn a_round_graph_covers_the_shown_round() {
        let mut history = History::new(1);
        for round in [0, 0, 0, 1, 1] {
            history.push(&snapshot(round, &[0.0], &[0.0]));
        }
        assert_eq!(history.range(5, false), 3..5);
        assert_eq!(history.range(2, false), 0..2);
        assert_eq!(history.range(4, true), 0..4);
    }
}
