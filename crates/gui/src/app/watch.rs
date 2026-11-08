// SPDX-License-Identifier: EPL-2.0

//! The pages of a battle: loading, opening a replay, watching it live or replayed.

use super::*;

impl App {
    /// The Start Battle tab while a battle starts, or the Watch Battle tab while a watched
    /// one connects (`remote`): the contestants, and what is going on with a way to cancel
    /// (or what went wrong, with the way back to the tab's page).
    pub(super) fn loading_page(&mut self, root: &mut egui::Ui, remote: bool) {
        let time = root.input(|i| i.time) as f32;
        let mut back = false;
        self.page(root, |app, ui| {
            let battle = if remote { &app.remote } else { &app.local };
            let Some(starting) = &battle.starting else {
                return;
            };
            let watching = starting.watching.as_deref();
            section(
                ui,
                match watching {
                    Some(_) => "Watching a battle",
                    None => "Starting the battle",
                },
            );
            ui.add_space(8.0);
            let known = &app.settings.robot_colors;
            contestants_row(ui, &mut app.sprites, &starting.contestants, known, time);
            ui.add_space(12.0);
            let (w, h) = starting.size;
            let mode = if starting.classic {
                "classic timing"
            } else {
                "deterministic"
            };
            let rounds = match starting.rounds {
                1 => "1 round".to_string(),
                n => format!("{n} rounds"),
            };
            ui.vertical_centered(|ui| {
                let known = watching.is_none() || !starting.contestants.is_empty();
                if known {
                    ui.label(
                        RichText::new(format!(
                            "{rounds} · {w} × {h} · {mode} · seed {}",
                            starting.seed
                        ))
                        .weak(),
                    );
                }
                if let Some(sandbox) = &starting.sandbox {
                    ui.label(
                        RichText::new(format!("robots in sandbox {sandbox}"))
                            .small()
                            .weak(),
                    );
                }
                if let Some(pin) = &starting.pin {
                    ui.label(
                        RichText::new(format!("TLS, certificate {pin}"))
                            .small()
                            .weak(),
                    );
                }
                ui.add_space(20.0);
                match &starting.error {
                    None => {
                        let what = match watching {
                            Some(a) if !known => format!("Connecting to {a}…"),
                            Some(a) => format!("Waiting for the battle on {a} to start…"),
                            None if starting.prepared.is_some() && docker_pulling() => {
                                "Pulling the container image (the first time, a few hundred MB)…"
                                    .into()
                            }
                            None if starting.prepared.is_some() => "Looking up the robots…".into(),
                            None => "Starting the robots…".into(),
                        };
                        ui.add(egui::Spinner::new().size(28.0).color(shell::ACCENT));
                        ui.add_space(8.0);
                        ui.label(what);
                        ui.add_space(16.0);
                        let button = egui::Button::new("Cancel").min_size(egui::vec2(120.0, 30.0));
                        if ui.add(button).clicked() {
                            back = true;
                        }
                    }
                    Some(e) => {
                        ui.label(
                            RichText::new(match watching {
                                Some(_) => "Couldn't watch the battle",
                                None => "The battle couldn't start",
                            })
                            .size(16.0)
                            .strong()
                            .color(Color32::LIGHT_RED),
                        );
                        ui.add_space(8.0);
                        egui::Frame::NONE
                            .fill(shell::BAR)
                            .stroke(egui::Stroke::new(1.0, shell::LINE))
                            .corner_radius(4.0)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.add(egui::Label::new(RichText::new(e).monospace()).wrap());
                                });
                            });
                        ui.add_space(16.0);
                        let back_to = match watching {
                            Some(_) => "Back",
                            None => "Back to the setup",
                        };
                        let button = egui::Button::new(RichText::new(back_to).strong())
                            .min_size(egui::vec2(170.0, 32.0));
                        if ui.add(button).clicked() {
                            back = true;
                        }
                    }
                }
                ui.add_space(8.0);
            });
        });
        if back {
            if remote {
                self.remote.close();
            } else {
                self.local.close();
            }
        }
    }

    /// The Watch Battle tab while a recording opens: its progress, and Cancel, which goes
    /// back to the replay or watched battle before, if any.
    pub(super) fn opening_page(&mut self, root: &mut egui::Ui) {
        let mut cancel = false;
        self.page(root, |app, ui| {
            let Some(opening) = &app.opening else {
                return;
            };
            let status = opening.status();
            section(
                ui,
                match status {
                    Some(_) => "Making a full recording",
                    None => "Opening the replay",
                },
            );
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(28.0).color(shell::ACCENT));
                ui.add_space(8.0);
                let name = opening
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                match status {
                    Some(status) => {
                        ui.label(RichText::new(name).weak());
                        ui.label(status);
                    }
                    None => {
                        ui.label(format!("Replaying {name}…"));
                    }
                }
                ui.add_space(10.0);
                let (bar, _) = ui.allocate_exact_size(egui::vec2(320.0, 6.0), egui::Sense::hover());
                let done = opening.progress().clamp(0.0, 1.0);
                let painter = ui.painter();
                painter.rect_filled(bar, 3.0, shell::LINE);
                let mut filled = bar;
                filled.set_width(bar.width() * done);
                painter.rect_filled(filled, 3.0, shell::ACCENT);
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{:.0}%", done * 100.0)).weak());
                ui.add_space(16.0);
                let button = egui::Button::new("Cancel").min_size(egui::vec2(120.0, 30.0));
                if ui.add(button).clicked() {
                    cancel = true;
                }
            });
        });
        // The progress moves on without input.
        root.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
        if cancel {
            self.opening = None;
        }
    }

    /// The Watch Battle tab with neither a replay nor a watched battle: a recording to
    /// open, or a battle on a server to watch.
    pub(super) fn watch_page(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        self.page(root, |app, ui| {
            section(ui, "From file");
            let mut open = false;
            egui::Grid::new("from-file")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Recording");
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut app.open_path)
                                .hint_text("from --record, \"Record to\" or the website")
                                .desired_width((ui.available_width() - 90.0).clamp(120.0, 420.0)),
                        );
                        if browse_button(ui, "Choose a recording") {
                            app.browse(Browse::Replay);
                        }
                        let enter =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let ready = !app.open_path.trim().is_empty();
                        let button = egui::Button::new("Open").min_size(egui::vec2(60.0, 0.0));
                        open = (ui.add_enabled(ready, button).clicked() || enter) && ready;
                    });
                    ui.end_row();
                    ui.label("");
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "A full recording (from --record or Start Battle's \"Record \
                                 to\") replays exactly, with debug paint and output. A website \
                                 replay (one round) shows what its viewer draws. Or drop a \
                                 file on the window.",
                            )
                            .weak(),
                        )
                        .wrap(),
                    );
                    ui.end_row();
                    if let Some(e) = &app.replay_error {
                        ui.label("");
                        ui.add(egui::Label::new(RichText::new(e).color(Color32::LIGHT_RED)).wrap());
                        ui.end_row();
                    }
                });
            if open {
                let path = PathBuf::from(app.open_path.trim());
                app.open_replay(ctx, path);
            }

            section(ui, "Remote server");
            let mut watch = false;
            egui::Grid::new("remote-server")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Address");
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut app.setup.watch)
                            .hint_text("host:port")
                            .desired_width(220.0),
                    );
                    watch |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    ui.label("Token");
                    ui.add(
                        egui::TextEdit::singleline(&mut app.setup.watch_token)
                            .password(true)
                            .hint_text("none")
                            .desired_width(220.0),
                    )
                    .on_hover_text("The battle's --observe-token, if it has one");
                    ui.end_row();
                    ui.label("Fingerprint");
                    let field = ui
                        .add(
                            egui::TextEdit::singleline(&mut app.setup.watch_fingerprint)
                                .hint_text("sha256:… (none on loopback)")
                                .desired_width(420.0),
                        )
                        .on_hover_text(
                            "The certificate fingerprint the battle prints with --observe \
                             (sha256:AB:CD:…): the connection uses TLS and trusts only \
                             that certificate. Off loopback, without one, the certificate \
                             must be a CA's.",
                        );
                    watch |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    ui.label("");
                    ui.horizontal(|ui| {
                        let button = egui::Button::new("Watch").min_size(egui::vec2(90.0, 0.0));
                        watch |= ui
                            .add(button)
                            .on_hover_text("Connect to the battle as an observer")
                            .clicked();
                    });
                    ui.end_row();
                    ui.label("");
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "A battle run with robocode-s battle --observe PORT, on this \
                                 machine or another (over TLS: give the fingerprint it \
                                 prints). Leaving it leaves the battle running.",
                            )
                            .weak(),
                        )
                        .wrap(),
                    );
                    ui.end_row();
                    if let Some(e) = &app.remote_error {
                        ui.label("");
                        ui.colored_label(Color32::LIGHT_RED, e);
                        ui.end_row();
                    }
                });
            if watch {
                app.watch(ctx);
            }
        });
    }

    /// The Start Battle tab with a battle, or the Watch Battle tab with a watched one
    /// (`remote`; see [`screen`]): the battlefield, the controls below it, the robots'
    /// output in tabs, and the robots on the right; at its end the results.
    pub(super) fn live_ui(&mut self, root: &mut egui::Ui, remote: bool) -> Next {
        let ctx = &root.ctx().clone();
        let mut next = Next::Stay;
        let style = self.style();
        let battle = if remote {
            &mut self.remote
        } else {
            &mut self.local
        };
        let Some(live) = &mut battle.live else {
            return Next::Setup;
        };
        let latest = live.latest.as_ref();
        screen::robots_panel(
            root,
            "robots",
            &live.robots,
            latest,
            &live.history,
            live.history.len(),
            &mut self.selected,
            true,
        );
        let (output, properties) = (&live.output, &live.properties);
        let event = screen::output_panel(
            root,
            "output",
            &live.robots,
            latest,
            &mut self.selected,
            &live.paint,
            |i| Cow::Borrowed(output.get(i).map_or("", String::as_str)),
            |i| properties.get(i).map_or_else(Cow::default, Cow::Borrowed),
            live.outcome.is_none() && !live.stopping,
        );
        let mut save = None;
        match event {
            Some(screen::OutputEvent::TogglePaint(i)) => {
                let on = !live.paint[i];
                live.set_paint(i, on);
            }
            Some(screen::OutputEvent::Save(i, text)) => {
                save = Some(screen::output_save(&live.robots[i], text));
            }
            Some(screen::OutputEvent::Clear(i)) => {
                if let Some(out) = live.output.get_mut(i) {
                    out.clear();
                }
            }
            Some(screen::OutputEvent::Kill(i)) => live.kill(i),
            None => {}
        }
        screen::controls_panel(root, "controls", |ui| {
            ui.horizontal(|ui| {
                let running = live.outcome.is_none() && !live.stopping;
                if screen::control(ui, running, "Stop the battle", |p, c, color| {
                    icons::stop(p, c, 6.0, color)
                }) {
                    live.stop();
                }
                let paused = live.paused;
                let hint = if paused { "Resume" } else { "Pause" };
                if screen::control(ui, running, hint, |p, c, color| {
                    if paused {
                        icons::play(p, c, 7.0, color);
                    } else {
                        icons::pause(p, c, 6.0, color);
                    }
                }) {
                    live.set_paused(!paused);
                }
                if screen::control(ui, running && paused, "One turn", |p, c, color| {
                    icons::step(p, c, 6.5, 1.0, color)
                }) {
                    live.step();
                }
                screen::divider(ui);
                let mut tps = live.turns_per_second;
                let (ended, stopping) = (live.outcome.is_some(), live.stopping);
                let watching = live.watching.is_some();
                let changed = screen::speed_and_readouts(
                    ui,
                    &mut tps,
                    Some(&live.seed.to_string()),
                    |ui| {
                        // A watched battle can be left as it runs (or when its server
                        // stopped answering): it goes on without us.
                        if watching && !ended {
                            let button = egui::Button::new("Leave").min_size(egui::vec2(0.0, 26.0));
                            if ui
                                .add(button)
                                .on_hover_text("Stop watching: the battle goes on without you")
                                .clicked()
                            {
                                next = Next::Setup;
                            }
                            ui.add_space(8.0);
                        }
                        if ended {
                            let back = if watching { "Done" } else { "New battle" };
                            let button = egui::Button::new(RichText::new(back).strong())
                                .min_size(egui::vec2(0.0, 26.0));
                            if ui.add(button).clicked() {
                                next = Next::Setup;
                            }
                            ui.add_space(8.0);
                        } else if stopping {
                            ui.label(RichText::new("Stopping…").weak());
                            ui.spinner();
                            ui.add_space(8.0);
                        }
                    },
                    |ui| {
                        if let Some(s) = &live.latest {
                            screen::info(ui, "Turn", s.turn.to_string());
                            ui.add_space(10.0);
                            screen::info(ui, "Round", format!("{} / {}", s.round + 1, live.rounds));
                        }
                    },
                    |_| {},
                );
                if changed {
                    live.set_speed(tps);
                }
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(root, |ui| {
                let field = Field {
                    width: live.width,
                    height: live.height,
                    robots: &live.robots,
                    sentry_border: live.sentry_border,
                };
                let available = ui.max_rect();
                let size = (live.width, live.height);
                let navigation = view::Navigation::Modified;
                view::navigate(ui, &mut live.camera, size, available, navigation);
                let (t, rect) = view::draw(
                    ui,
                    &mut self.sprites,
                    &field,
                    style,
                    (live.previous.as_ref(), live.latest.as_ref()),
                    available,
                    &live.camera,
                );
                if live.debug {
                    ui.painter().text(
                        available.left_top() + egui::vec2(10.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        "Debug battle: robots from your IDE aren't timed",
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(255, 190, 90),
                    );
                }
                if live.outcome.is_none() && !live.stopping {
                    let layer = ui.layer_id();
                    for event in live.interaction.events(ui.ctx(), layer, rect, &t) {
                        live.input(event);
                    }
                }
            });
        match &live.outcome {
            Some(Ok(outcome)) => {
                let title = results_title(outcome);
                let shown = screen::Outcome {
                    title: &title,
                    robots: &live.robots,
                    results: &outcome.results,
                    snapshot: live.latest.as_ref(),
                    stopped: outcome.aborted,
                };
                let id = egui::Id::new(("results", self.battles));
                let sprites = &mut self.sprites;
                let request = screen::results_window(ctx, sprites, id, &shown, |ui| {
                    if live.watching.is_some() {
                        if ui
                            .add(egui::Button::new(RichText::new("Done").strong()))
                            .on_hover_text("Back to Watch Battle.")
                            .clicked()
                        {
                            next = Next::Setup;
                        }
                        return;
                    }
                    let replays = if !live.deterministic {
                        "Classic mode: the robots' timing differs, so it may go differently."
                    } else {
                        "Deterministic mode: the battle repeats exactly."
                    };
                    let restart = egui::Button::new(RichText::new("Restart Battle").strong());
                    if ui
                        .add(restart)
                        .on_hover_text(format!(
                            "The same robots and settings, with seed {}. {replays}",
                            live.seed
                        ))
                        .clicked()
                    {
                        next = Next::Restart { new_seed: false };
                    }
                    if ui
                        .button("Restart, new seed")
                        .on_hover_text("The same robots and settings with another seed.")
                        .clicked()
                    {
                        next = Next::Restart { new_seed: true };
                    }
                    if ui
                        .button("New Battle")
                        .on_hover_text("Back to the setup, with these robots.")
                        .clicked()
                    {
                        next = Next::Setup;
                    }
                });
                if request.is_some() {
                    save = request;
                }
            }
            Some(Err(e)) => {
                egui::Window::new("Battle failed").show(ctx, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, e);
                    if ui.button("Back").clicked() {
                        next = Next::Setup;
                    }
                });
            }
            None => {}
        }
        if let Some(save) = save {
            self.save(save);
        }
        next
    }
}

/// The contestants on the loading screen: their tanks with their names, the radars turning
/// while they start. The tanks are in the colours a robot had in the last battle seen with
/// it (`known`), else in those it gets when it sets none: its own are only known once the
/// battle runs.
pub(super) fn contestants_row(
    ui: &mut egui::Ui,
    sprites: &mut Sprites,
    names: &[String],
    known: &RobotColors,
    time: f32,
) {
    const CELL: egui::Vec2 = egui::vec2(132.0, 92.0);
    let per_row = ((ui.available_width() / CELL.x).floor() as usize).max(1);
    let rows = names.len().div_ceil(per_row).max(1);
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), rows as f32 * CELL.y),
        egui::Sense::hover(),
    );
    let ctx = ui.ctx().clone();
    for (row, chunk) in names.chunks(per_row).enumerate() {
        let left = area.center().x - chunk.len() as f32 * CELL.x / 2.0;
        for (k, name) in chunk.iter().enumerate() {
            let i = row * per_row + k;
            let cell = egui::Rect::from_min_size(
                egui::pos2(left + k as f32 * CELL.x, area.top() + row as f32 * CELL.y),
                CELL,
            );
            let center = egui::pos2(cell.center().x, cell.top() + 32.0);
            // Default colours come from the class name, without version or duplicate suffix,
            // or the `*` of a robot from a class folder.
            let class = name.split(' ').next().unwrap_or(name).trim_end_matches('*');
            let colors = known
                .get(name)
                .unwrap_or_else(|| crate::sprites::default_colors(class));
            let radar = time * 2.0 + i as f32 * 0.7;
            for (part, color, heading) in [
                (Part::Body, colors[0], 0.0),
                (Part::Gun, colors[1], 0.0),
                (Part::Radar, colors[2], radar),
            ] {
                let (texture, size) = sprites.tank(&ctx, part, color);
                let shape = view::sprite(texture, center, size * 1.25, heading, Color32::WHITE);
                ui.painter().add(shape);
            }
            let label = egui::Rect::from_min_size(
                egui::pos2(cell.left() + 4.0, cell.top() + 62.0),
                egui::vec2(CELL.x - 8.0, 20.0),
            );
            ui.put(
                label,
                egui::Label::new(RichText::new(name).strong()).truncate(),
            );
        }
    }
    crate::pace::next_frame(ui.ctx());
}

/// The results window's title: a stopped battle says in which round.
pub(super) fn results_title(outcome: &BattleOutcome) -> String {
    if outcome.aborted {
        match outcome.rounds {
            1 => "Results: stopped in the first round".to_string(),
            n => format!("Results: stopped in round {n}"),
        }
    } else {
        "Results".to_string()
    }
}
