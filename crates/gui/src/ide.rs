// SPDX-License-Identifier: EPL-2.0

//! Robots from the user's IDE (#17, design §10): while the app is open it listens on the
//! IDE port (`server::ide`); a robot started by the IDE's debugger describes itself and
//! waits there. It shows in a pinned group of the available robots, with a notice, and
//! the "From your IDE…" panel has the run configuration to copy. Added to a battle it is
//! an entry `@ide:ID` ([`robocode_s_runner::ide::marker`]) that the battle takes from the
//! listener when it starts.

use crate::settings::Settings;
use crate::shell;
use eframe::egui::{self, Color32, RichText};
use robocode_s_runner::ide::{RunConfiguration, marker, parse_marker, run_configuration};
use robocode_s_runner::local::Environment;
use robocode_s_server::ide::{IdeListener, IdeRobot, token};
use std::collections::{HashMap, HashSet};

/// How long a robot's notice shows, in seconds.
const NOTICE_SECONDS: f64 = 15.0;

/// A robot that just connected, announced until the user acts on it or it times out.
pub struct Notice {
    pub id: u64,
    pub name: String,
    since: f64,
}

#[derive(Default)]
pub struct Ide {
    listener: Option<IdeListener>,
    /// What the listener was made for: whether to listen, and the port.
    made_for: Option<(bool, u16)>,
    /// Why it isn't listening.
    pub error: Option<String>,
    /// The robots waiting now, oldest first.
    pub connected: Vec<IdeRobot>,
    /// The names of the robots seen, for entries whose robot has gone.
    names: HashMap<u64, String>,
    announced: HashSet<u64>,
    pub notice: Option<Notice>,
    /// The "From your IDE…" panel is open.
    pub open: bool,
    configuration: Option<(u16, Result<RunConfiguration, String>)>,
}

/// How a battle list entry shows: the robot's name and whether its robot is connected;
/// `None` for an entry that isn't a robot from an IDE.
pub fn entry_state(
    entry: &str,
    connected: &[IdeRobot],
    names: &HashMap<u64, String>,
) -> Option<(String, bool)> {
    let id = parse_marker(entry)?;
    let here = connected.iter().find(|r| r.id == id);
    let name = here
        .map(|r| r.description.name.clone())
        .or_else(|| names.get(&id).cloned())
        .unwrap_or_else(|| "A robot from your IDE".into());
    Some((name, here.is_some()))
}

/// The name of a robot from an IDE in lists.
pub fn label(name: &str) -> String {
    format!("{name} · IDE")
}

/// The first entry of `robots` that is a robot from an IDE which isn't connected, with its
/// name.
pub fn disconnected(
    robots: &[String],
    connected: &[IdeRobot],
    names: &HashMap<u64, String>,
) -> Option<String> {
    robots
        .iter()
        .filter_map(|r| entry_state(r, connected, names))
        .find(|(_, here)| !here)
        .map(|(name, _)| name)
}

impl Ide {
    /// `open`: the panel is open from the start.
    pub fn new(open: bool) -> Self {
        Self {
            open,
            ..Self::default()
        }
    }

    /// The listener, for battles that take robots from it.
    pub fn listener(&self) -> Option<IdeListener> {
        self.listener.clone()
    }

    /// The state of a battle list entry (see [`entry_state`]).
    pub fn state(&self, entry: &str) -> Option<(String, bool)> {
        entry_state(entry, &self.connected, &self.names)
    }

    /// An entry as the battle's lists name it.
    pub fn display(&self, entry: &str) -> String {
        match self.state(entry) {
            Some((name, _)) => label(&name),
            None => entry.to_string(),
        }
    }

    /// The first robot of `robots` from an IDE that isn't connected (its name).
    pub fn missing(&self, robots: &[String]) -> Option<String> {
        disconnected(robots, &self.connected, &self.names)
    }

    /// Follows the options (listen, port), takes the robots that are here and announces
    /// new ones. Every frame.
    pub fn sync(&mut self, ctx: &egui::Context, env: &Environment, settings: &Settings) {
        let want = (settings.ide_listen, settings.ide_port);
        if self.made_for != Some(want) {
            self.made_for = Some(want);
            self.listen(env, want);
        }
        let Some(listener) = &self.listener else {
            self.connected.clear();
            return;
        };
        self.connected = listener.robots();
        let now = ctx.input(|i| i.time);
        for robot in &self.connected {
            self.names.insert(robot.id, robot.description.name.clone());
            if self.announced.insert(robot.id) {
                self.notice = Some(Notice {
                    id: robot.id,
                    name: robot.description.name.clone(),
                    since: now,
                });
            }
        }
        if let Some(notice) = &self.notice
            && (now - notice.since > NOTICE_SECONDS
                || !self.connected.iter().any(|r| r.id == notice.id))
        {
            self.notice = None;
        }
        // Nothing wakes the app when a robot connects.
        let wait = if self.open || self.notice.is_some() {
            0.25
        } else {
            1.0
        };
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
    }

    /// Starts (or stops) listening for `(listen, port)`.
    fn listen(&mut self, env: &Environment, (listen, port): (bool, u16)) {
        if let Some(old) = self.listener.take() {
            old.close();
        }
        self.error = None;
        if !listen {
            return;
        }
        let address = format!("127.0.0.1:{port}");
        let bound = token(&env.data)
            .map_err(|e| format!("The IDE token couldn't be made: {e}"))
            .and_then(|token| {
                IdeListener::bind(&address, token).map_err(|e| {
                    format!(
                        "Can't listen on {address}: {e}. Another program may be using the port \
                         (perhaps another robocode-s): choose another in the options."
                    )
                })
            });
        match bound {
            Ok(listener) => self.listener = Some(listener),
            Err(e) => self.error = Some(e),
        }
    }

    /// Listens again (after the user freed the port).
    pub fn retry(&mut self) {
        self.made_for = None;
    }

    /// The notice for the robot that connected last: the id of the robot if "Add to
    /// battle" was clicked.
    pub fn notice_ui(&mut self, ctx: &egui::Context, bottom: f32) -> Option<u64> {
        let notice = self.notice.as_ref()?;
        let (id, name) = (notice.id, notice.name.clone());
        let mut add = false;
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("ide-notice"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -bottom])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(shell::BAR)
                    .stroke(egui::Stroke::new(1.0, shell::ACCENT))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(label(&name)).strong());
                            ui.label("connected from your IDE");
                            if ui.button("Add to battle").clicked() {
                                add = true;
                            }
                            if ui.small_button("x").on_hover_text("Dismiss").clicked() {
                                dismiss = true;
                            }
                        });
                    });
            });
        if add || dismiss {
            self.notice = None;
        }
        add.then_some(id)
    }

    /// The "From your IDE…" panel: what to set up in the IDE, and the robots that are
    /// here. `in_battle` holds the battle's entries. Returns the robot to add to the battle.
    pub fn panel(
        &mut self,
        ctx: &egui::Context,
        env: &Environment,
        settings: &mut Settings,
        in_battle: &[String],
    ) -> (Option<u64>, bool) {
        if !self.open {
            return (None, false);
        }
        let port = settings.ide_port;
        if !matches!(&self.configuration, Some((p, _)) if *p == port) {
            self.configuration = Some((port, run_configuration(env, port)));
        }
        let mut open = true;
        let mut add = None;
        let mut changed = false;
        let mut retry = false;
        egui::Window::new("From your IDE")
            .open(&mut open)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(680.0)
            .min_width(560.0)
            .max_height((ctx.content_rect().height() - 90.0).max(200.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([true, true])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                "Debug a robot with your IDE's own debugger: the IDE starts the \
                             robot (its Debug button), and the robot waits here until you add \
                             it to a battle. A breakpoint pauses the whole battle.",
                            )
                            .wrap(),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            changed |= ui
                                .checkbox(&mut settings.ide_listen, "Listen on 127.0.0.1, port")
                                .changed();
                            changed |= ui
                                .add(egui::DragValue::new(&mut settings.ide_port).range(1..=65535))
                                .changed();
                        });
                        match (&self.error, &self.listener) {
                            (Some(e), _) => {
                                ui.add(
                                    egui::Label::new(RichText::new(e).color(Color32::LIGHT_RED))
                                        .wrap(),
                                );
                                retry |= ui.button("Try again").clicked();
                            }
                            (None, Some(_)) => {
                                ui.label(RichText::new("Listening: robots can connect.").weak());
                            }
                            (None, None) => {
                                ui.label(
                                    RichText::new(
                                        "Not listening: robots from your IDE can't connect.",
                                    )
                                    .weak(),
                                );
                            }
                        }

                        ui.add_space(10.0);
                        ui.label(RichText::new("Robots connected now").strong());
                        if self.connected.is_empty() {
                            ui.label(
                                RichText::new("None yet: start your robot from the IDE.").weak(),
                            );
                        }
                        for robot in &self.connected {
                            let entry = marker(robot.id);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(label(&robot.description.name)).strong());
                                if !robot.description.platform.is_empty() {
                                    ui.label(RichText::new(&robot.description.platform).weak());
                                }
                                if in_battle.contains(&entry) {
                                    ui.label(RichText::new("in the battle").weak());
                                } else if ui.button("Add to battle").clicked() {
                                    add = Some(robot.id);
                                }
                            });
                        }

                        ui.add_space(10.0);
                        ui.label(RichText::new("Run configuration").strong());
                        match &self.configuration {
                            Some((_, Ok(c))) => configuration_ui(ui, c),
                            Some((_, Err(e))) => {
                                ui.colored_label(Color32::LIGHT_RED, e);
                            }
                            None => {}
                        }
                    });
            });
        self.open = open;
        if retry {
            self.retry();
        }
        (add, changed)
    }
}

/// The run configuration's parts, each with a button to copy it, then the whole text.
fn configuration_ui(ui: &mut egui::Ui, c: &RunConfiguration) {
    let rows = [
        ("Main class", c.main_class.clone()),
        ("Classpath", c.classpath()),
        ("VM options", c.vm_options_line()),
        ("Program arguments", c.arguments_line()),
        ("Environment", format!("{}={}", c.env_name, c.env_value)),
    ];
    for (name, value) in rows {
        ui.horizontal(|ui| {
            ui.label(name);
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(value.clone());
            }
        });
        ui.add(egui::Label::new(RichText::new(&value).monospace().small()).wrap());
        ui.add_space(4.0);
    }
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new(
                "Replace my.Robot with your robot's class name and <output-folder> with the \
                 folder your IDE compiles to. The classpath is every jar in that folder, besides \
                 the project's own. The port and the token stay the same, so this is set up once.",
            )
            .weak()
            .small(),
        )
        .wrap(),
    );
    ui.horizontal_wrapped(|ui| {
        if ui.button("Copy for Visual Studio Code").clicked() {
            ui.ctx().copy_text(c.vscode());
        }
        if ui.button("Copy everything").clicked() {
            ui.ctx().copy_text(c.text());
        }
        if ui
            .button("Copy -javaagent option")
            .on_hover_text("Optional: seeded random numbers (add --seed N, the battle's seed)")
            .clicked()
        {
            ui.ctx().copy_text(c.agent_option());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use robocode_s_protocol::v1;

    fn robot(id: u64, name: &str) -> IdeRobot {
        IdeRobot {
            id,
            description: v1::RobotDescription {
                name: name.into(),
                ..Default::default()
            },
            peer: "127.0.0.1:1".parse().unwrap(),
        }
    }

    #[test]
    fn entries_say_whether_their_robot_is_connected() {
        let here = [robot(3, "my.Robot*")];
        let names = HashMap::from([(3, "my.Robot*".to_string()), (4, "gone.Robot".to_string())]);
        assert_eq!(entry_state("sample.Fire", &here, &names), None);
        assert_eq!(
            entry_state(&marker(3), &here, &names),
            Some(("my.Robot*".into(), true))
        );
        assert_eq!(
            entry_state(&marker(4), &here, &names),
            Some(("gone.Robot".into(), false))
        );
        assert_eq!(
            entry_state(&marker(5), &here, &names).map(|(_, here)| here),
            Some(false)
        );
    }

    #[test]
    fn start_names_the_first_robot_that_left() {
        let here = [robot(1, "a.A")];
        let names = HashMap::from([(1, "a.A".to_string()), (2, "b.B".to_string())]);
        let ok = ["sample.Fire".to_string(), marker(1)];
        assert_eq!(disconnected(&ok, &here, &names), None);
        let gone = [marker(1), marker(2), "sample.Fire".to_string()];
        assert_eq!(disconnected(&gone, &here, &names), Some("b.B".into()));
    }

    #[test]
    fn lists_name_them_with_the_ide() {
        assert_eq!(label("my.Robot*"), "my.Robot* · IDE");
    }
}
