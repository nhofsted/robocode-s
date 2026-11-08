// SPDX-License-Identifier: EPL-2.0

//! The start page: the robot chooser, the contestants and the game.

use super::*;

impl App {
    /// The Start Battle tab without a battle: the setup.
    pub(super) fn start_page(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        self.page(root, |app, ui| {
            section(ui, "Robots");
            app.robot_chooser(ui);

            section(ui, "Battle");
            let mut browse = None;
            let mut save_battle = false;
            let s = &mut app.setup;
            // The options in two columns, lined up with the robot lists, when they fit.
            const GAP: f32 = 24.0;
            let width = (ui.available_width() - GAP) / 2.0;
            if width >= 400.0 {
                ui.horizontal_top(|ui| {
                    let layout = egui::Layout::top_down(egui::Align::Min);
                    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), layout, |ui| {
                        ui.set_width(width);
                        options_grid(ui, "setup-game", |ui| {
                            game_rows(ui, s, &mut browse, &mut save_battle);
                        });
                    });
                    ui.add_space(GAP - ui.spacing().item_spacing.x);
                    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), layout, |ui| {
                        ui.set_width(width);
                        options_grid(ui, "setup-rules", |ui| {
                            rules_and_output_rows(ui, s, &mut browse);
                        });
                    });
                });
            } else {
                options_grid(ui, "setup", |ui| {
                    game_rows(ui, s, &mut browse, &mut save_battle);
                    rules_and_output_rows(ui, s, &mut browse);
                });
            }
            if let Some(field) = browse {
                app.browse(field);
            }
            if save_battle {
                app.save_battle();
            }

            ui.add_space(20.0);
            ui.horizontal(|ui| {
                let jars = format!(
                    "Robots from {} in the robot folders (see Options){}.",
                    sources(&app.env.jars),
                    if app.env_jars.is_empty() {
                        ""
                    } else {
                        " and ROBOCODES_ROBOT_JARS"
                    }
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let start = egui::Button::new(
                        RichText::new("Start Battle")
                            .size(16.0)
                            .strong()
                            .color(shell::BAR),
                    )
                    .fill(shell::ACCENT)
                    .min_size(egui::vec2(170.0, 40.0));
                    let ready = !app.setup.robots.is_empty();
                    if ui.add_enabled(ready, start).clicked() {
                        app.start(ctx);
                    }
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        match &app.error {
                            Some(e) => ui.colored_label(Color32::LIGHT_RED, e),
                            None => ui.label(RichText::new(jars).weak()),
                        };
                    });
                });
            });
        });
    }

    /// The robots: the available ones with their filter on the left, the battle's on the
    /// right. A double-click (or the row's plus) adds a robot, again for another instance;
    /// a double-click (or the row's cross) removes it.
    pub(super) fn robot_chooser(&mut self, ui: &mut egui::Ui) {
        const GAP: f32 = 24.0;
        let width = ((ui.available_width() - GAP) / 2.0).max(160.0);
        // The lists take what the window has left over the rest of the page, within limits.
        let lists = (ui.ctx().content_rect().height() - 485.0).clamp(96.0, 300.0);
        let column = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(width);
                    add(ui);
                },
            );
        };
        let mut add = None;
        let mut field_height = ui.spacing().interact_size.y;
        ui.horizontal_top(|ui| {
            column(ui, &mut |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Filter the robots")
                        .desired_width(width),
                );
                field_height = field.rect.height();
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                list_box(ui, "available", lists, |ui| {
                    // Robots from the IDE: pinned at the top, whatever the filter says.
                    if !self.ide.connected.is_empty() {
                        ui.label(
                            RichText::new("FROM YOUR IDE")
                                .small()
                                .strong()
                                .color(shell::ACCENT),
                        );
                        for r in &self.ide.connected {
                            let entry = marker(r.id);
                            let in_battle = self.setup.robots.contains(&entry);
                            let tag = in_battle.then_some("in the battle");
                            let picked = self.picked.as_deref() == Some(entry.as_str());
                            let name = ide::label(&r.description.name);
                            let row = robot_row(ui, &name, tag, picked, Action::Add);
                            if row.clicked {
                                self.picked = Some(entry.clone());
                            }
                            if row.act && !in_battle {
                                add = Some(entry);
                            }
                        }
                        ui.add_space(6.0);
                    }
                    match &self.available {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new("Listing the robots…").weak());
                            });
                        }
                        Some(Err(e)) => {
                            ui.colored_label(Color32::LIGHT_RED, e);
                        }
                        Some(Ok(robots)) => {
                            let filter = self.filter.to_lowercase();
                            let shown: Vec<&Listed> = robots
                                .iter()
                                .filter(|r| r.name.to_lowercase().contains(&filter))
                                .collect();
                            if shown.is_empty() {
                                let none = if robots.is_empty() {
                                    "No robots: add a robot folder in the options."
                                } else {
                                    "No robot matches the filter."
                                };
                                ui.label(RichText::new(none).weak());
                            }
                            for r in &shown {
                                let picked = self.picked.as_deref() == Some(r.name.as_str());
                                let row = robot_row(
                                    ui,
                                    &r.name,
                                    r.team.then_some("team"),
                                    picked,
                                    Action::Add,
                                );
                                if row.clicked {
                                    self.picked = Some(r.name.clone());
                                    self.details = Some(r.name.clone());
                                }
                                if row.act {
                                    add = Some(r.name.clone());
                                }
                            }
                            if enter {
                                // Enter adds the highlighted robot, else the first match.
                                let picked = shown
                                    .iter()
                                    .find(|r| self.picked.as_deref() == Some(r.name.as_str()))
                                    .or(shown.first());
                                add = picked.map(|r| r.name.clone());
                                field.request_focus();
                            }
                        }
                    }
                });
            });
            ui.add_space(GAP - ui.spacing().item_spacing.x);
            column(ui, &mut |ui| {
                let robots = &mut self.setup.robots;
                let height = field_height;
                ui.allocate_ui_with_layout(
                    egui::vec2(width, height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_height(height);
                        let count = match robots.len() {
                            0 => "In the battle".to_string(),
                            n => format!("In the battle ({n})"),
                        };
                        ui.label(RichText::new(count).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(!robots.is_empty(), egui::Button::new("Clear"))
                                .clicked()
                            {
                                robots.clear();
                                self.picked_selected = None;
                            }
                            if ui
                                .button("From your IDE…")
                                .on_hover_text(
                                    "Debug a robot with your IDE's debugger: how to set it up, \
                                     and the robots waiting",
                                )
                                .clicked()
                            {
                                self.ide.open = true;
                            }
                        });
                    },
                );
                ui.add_space(6.0);
                let mut remove = None;
                list_box(ui, "selected", lists, |ui| {
                    if robots.is_empty() {
                        ui.label(RichText::new("Double-click a robot to add it.").weak());
                    }
                    for (i, name) in robots.iter().enumerate() {
                        let from_ide = self.ide.state(name);
                        let team = matches!(&self.available,
                            Some(Ok(a)) if a.iter().any(|r| r.team && &r.name == name));
                        let picked = self.picked_selected == Some(i);
                        let (shown, tag) = match &from_ide {
                            Some((robot, true)) => (ide::label(robot), Some("waiting")),
                            Some((robot, false)) => (ide::label(robot), Some("not connected")),
                            None => (name.clone(), team.then_some("team")),
                        };
                        let row = robot_row(ui, &shown, tag, picked, Action::Remove);
                        if row.clicked {
                            self.picked_selected = Some(i);
                            self.details = Some(name.clone());
                        }
                        if row.act {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    robots.remove(i);
                    self.picked_selected = None;
                }
            });
        });
        if let Some(name) = add {
            self.add_robot(name);
        }
        self.robot_details(ui);
    }

    /// Adds a robot to the battle; a robot from an IDE (one connection) only once.
    pub(super) fn add_robot(&mut self, name: String) {
        if parse_marker(&name).is_none() || !self.setup.robots.contains(&name) {
            self.setup.robots.push(name);
        }
    }

    /// The details of the robot clicked last, from its `.properties`: its name and
    /// version, author, description and web page.
    pub(super) fn robot_details(&self, ui: &mut egui::Ui) {
        let Some(Ok(available)) = &self.available else {
            return;
        };
        let Some(robot) = self
            .details
            .as_ref()
            .and_then(|name| available.iter().find(|r| &r.name == name))
        else {
            return;
        };
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&robot.name).strong());
            if !robot.author.is_empty() {
                ui.label(RichText::new(format!("by {}", robot.author)).weak());
            }
            if !robot.webpage.is_empty() {
                ui.hyperlink_to(&robot.webpage, &robot.webpage);
            }
        });
        let description = if robot.description.is_empty() {
            RichText::new("No description.").weak().italics()
        } else {
            RichText::new(&robot.description).weak()
        };
        ui.add(egui::Label::new(description).wrap());
        let facts = robot_facts(robot);
        if !facts.is_empty() {
            ui.label(RichText::new(facts).weak());
        }
    }
}

/// A framed, scrolling list of robots, as wide as the column (so its scrollbar lines up
/// with the field above it).
pub(super) fn list_box(
    ui: &mut egui::Ui,
    id: &str,
    height: f32,
    content: impl FnOnce(&mut egui::Ui),
) {
    list_frame(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink([false, false])
            .min_scrolled_height(height)
            .max_height(height)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                content(ui);
            });
    });
}

/// What a robot row's icon does.
#[derive(Clone, Copy)]
pub(super) enum Action {
    Add,
    Remove,
}

/// What happened to a robot row: clicked (it is highlighted), or its action asked for (a
/// double-click, or a click on its icon).
pub(super) struct Row {
    pub(super) clicked: bool,
    pub(super) act: bool,
}

/// A row of a robot list, as wide as the list: the name, a small tag on the right ("team"
/// for a team, the state of a robot from an IDE), and the action's icon when the row is
/// hovered or highlighted.
pub(super) fn robot_row(
    ui: &mut egui::Ui,
    name: &str,
    tag: Option<&str>,
    picked: bool,
    action: Action,
) -> Row {
    const HEIGHT: f32 = 24.0;
    // Room on the right for the scrollbar, which floats over the rows.
    const RIGHT: f32 = 10.0;
    let size = egui::vec2(ui.available_width(), HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let hint = match action {
        Action::Add => "Double-click to add (again for another one)",
        Action::Remove => "Double-click to remove",
    };
    let response = response.on_hover_text_at_pointer(hint);
    let visuals = ui.visuals();
    let hovered = response.hovered();
    let painter = ui.painter_at(rect);
    if picked || hovered {
        let fill = if picked {
            visuals.selection.bg_fill.gamma_multiply(0.6)
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(rect, 3.0, fill);
    }
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.right() - RIGHT - 10.0, rect.center().y),
        egui::vec2(20.0, 20.0),
    );
    let on_icon = response
        .interact_pointer_pos()
        .or(response.hover_pos())
        .is_some_and(|p| icon.contains(p));
    let mut right = icon.left() - 4.0;
    if let Some(tag) = tag {
        let tag = painter.text(
            egui::pos2(right, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            tag,
            egui::FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
        right = tag.left() - 6.0;
    }
    let galley = painter.layout_no_wrap(
        name.to_string(),
        egui::FontId::proportional(14.0),
        visuals.text_color(),
    );
    let at = egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0);
    let clip = egui::Rect::from_min_max(rect.min, egui::pos2(right, rect.max.y));
    painter
        .with_clip_rect(clip)
        .galley(at, galley, visuals.text_color());
    if picked || hovered {
        let color = if on_icon {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        if on_icon {
            painter.rect_filled(icon, 3.0, visuals.widgets.active.weak_bg_fill);
        }
        match action {
            Action::Add => icons::plus(&painter, icon.center(), 5.0, color),
            Action::Remove => icons::cross(&painter, icon.center(), 4.0, color),
        }
    }
    let clicked = response.clicked();
    Row {
        clicked,
        act: response.double_clicked() || (clicked && on_icon),
    }
}
