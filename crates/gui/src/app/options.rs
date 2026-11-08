// SPDX-License-Identifier: EPL-2.0

//! The options page, and the rows of the game and its rules it shares with the start page.

use super::*;

impl App {
    /// The options, behind the gear: robot folders, how battles are drawn, and the
    /// defaults for new battles. Every change is saved at once.
    pub(super) fn options_page(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        let before = self.settings.clone();
        let mut rescan = false;
        self.page(root, |app, ui| {
            ui.heading("Options");
            ui.add_space(4.0);
            match Settings::path() {
                Some(path) => ui.label(
                    RichText::new(format!("Saved in {}", path.display()))
                        .small()
                        .weak(),
                ),
                None => ui.label(RichText::new("Not saved: no config directory").small()),
            };

            section(ui, "Robots");
            egui::Grid::new("robot-folders")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    // One row per folder, the label on the first.
                    let mut remove = None;
                    for (i, folder) in app.settings.robot_folders.iter().enumerate() {
                        ui.label(if i == 0 { "Robot folders" } else { "" });
                        ui.horizontal(|ui| {
                            let hint = "Remove this folder";
                            if icons::button(ui, 20.0, hint, false, |p, c, color| {
                                icons::cross(p, c, 4.0, color);
                            })
                            .clicked()
                            {
                                remove = Some(i);
                            }
                            ui.label(RichText::new(folder.display().to_string()).monospace());
                            match app.folder_contents.get(i).cloned().flatten() {
                                Some(c) => ui.label(RichText::new(holds(&c)).weak()),
                                None => ui.colored_label(Color32::LIGHT_RED, "not found"),
                            };
                        });
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        app.settings.robot_folders.remove(i);
                    }
                    if app.settings.robot_folders.is_empty() {
                        ui.label("Robot folders");
                        ui.label(
                            RichText::new("None yet: add a folder with robot jars or classes.")
                                .weak(),
                        );
                        ui.end_row();
                    }
                    ui.label("");
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut app.new_folder)
                                .hint_text("robot jars, or an IDE's output (e.g. bin)")
                                .desired_width((ui.available_width() - 90.0).clamp(120.0, 420.0)),
                        );
                        if browse_button(ui, "Choose a folder with robot jars or classes") {
                            app.browse(Browse::Folder);
                        }
                        let enter =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let text = app.new_folder.trim();
                        let add = ui.add_enabled(!text.is_empty(), egui::Button::new("Add"));
                        if (add.clicked() || enter) && !text.is_empty() {
                            let folder = expand_home(text);
                            if !app.settings.robot_folders.contains(&folder) {
                                app.settings.robot_folders.push(folder);
                            }
                            app.new_folder.clear();
                        }
                    });
                    ui.end_row();

                    ui.label("Found");
                    ui.horizontal(|ui| {
                        let robots = match &app.available {
                            None => "listing robots…".to_string(),
                            Some(Ok(robots)) => format!("{} robots", robots.len()),
                            Some(Err(_)) => "the robots couldn't be listed".to_string(),
                        };
                        ui.label(format!("{}, {robots}", sources(&app.env.jars)));
                        if app.listing.is_some() {
                            ui.spinner();
                        }
                        if ui
                            .button("Rescan")
                            .on_hover_text("Look for new robots in the folders")
                            .clicked()
                        {
                            rescan = true;
                        }
                    });
                    ui.end_row();
                    if let Some(Err(e)) = &app.available {
                        ui.label("");
                        ui.colored_label(Color32::LIGHT_RED, e);
                        ui.end_row();
                    }
                    if !app.env_jars.is_empty() {
                        ui.label("Also");
                        ui.label(
                            RichText::new(format!(
                                "{} from ROBOCODES_ROBOT_JARS",
                                sources(&app.env_jars)
                            ))
                            .weak(),
                        );
                        ui.end_row();
                    }
                    sandbox_row(ui, &mut app.settings);
                    ui.label("Warm robots").on_hover_text(
                        "Keeps the JVMs of up to two of the next battle's robots started \
                         while you watch and between battles, so that it starts sooner \
                         (about half a second less). Each idle JVM takes about 90 MB.",
                    );
                    ui.checkbox(&mut app.settings.warm_hosts, "Keep robot JVMs started");
                    ui.end_row();
                    ui.label("From your IDE").on_hover_text(
                        "Robots started by your IDE's debugger connect to this port on this \
                         computer and wait for a battle (Start Battle, From your IDE…).",
                    );
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut app.settings.ide_listen, "Listen on 127.0.0.1, port");
                        ui.add(egui::DragValue::new(&mut app.settings.ide_port).range(1..=65535));
                    });
                    ui.end_row();
                    if let Some(e) = &app.ide.error {
                        ui.label("");
                        ui.colored_label(Color32::LIGHT_RED, e);
                        ui.end_row();
                    }
                });

            section(ui, "Battlefield");
            let s = &mut app.settings;
            egui::Grid::new("view-options")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Scan arcs");
                    ui.checkbox(&mut s.scan_arcs, "Show each robot's radar sweep");
                    ui.end_row();
                    ui.label("Coloured bullets");
                    ui.checkbox(
                        &mut s.colored_bullets,
                        "Bullets in their robot's bullet colour",
                    );
                    ui.end_row();
                    ui.label("Robots");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.robot_names, "Names");
                        ui.checkbox(&mut s.robot_energy, "Energy");
                    });
                    ui.end_row();
                    ui.label("Explosions");
                    ui.checkbox(&mut s.explosions, "Show hits and deaths exploding");
                    ui.end_row();
                    ui.label("Ground");
                    ui.checkbox(&mut s.ground, "Draw the floor tiles");
                    ui.end_row();
                    ui.label("Sentry border");
                    ui.checkbox(&mut s.sentry_border, "Show it when a sentry robot fights");
                    ui.end_row();
                    ui.label("Speed");
                    ui.checkbox(&mut s.rates, "Show the turns and frames per second");
                    ui.end_row();
                    ui.label("Frame rate");
                    ui.horizontal(|ui| {
                        for fps in crate::pace::RATES {
                            ui.radio_value(&mut s.frame_rate, fps, crate::pace::label(fps));
                        }
                    })
                    .response
                    .on_hover_text(
                        "The most frames a second for what moves: the background, \
                         battles, replays. Display: as many as the screen shows. A battle \
                         draws a frame per turn, so at most as many as its turns a second.",
                    );
                    ui.end_row();
                    ui.label("Background");
                    ui.checkbox(&mut s.background, "Tanks wandering behind the pages")
                        .on_hover_text(
                            "Off, they stand still, and the app draws nothing while nothing \
                             else moves.",
                        );
                    ui.end_row();
                    ui.label("Sound");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.sound, "Gunshots, hits and explosions")
                            .on_hover_text(format!(
                                "Classic's sound effects, up to {} turns per second.",
                                sound::MAX_TPS
                            ));
                        ui.add_enabled(
                            s.sound,
                            egui::Slider::new(&mut s.volume, 0.0..=1.0)
                                .show_value(false)
                                .text("Volume"),
                        );
                    });
                    ui.end_row();
                    if s.sound
                        && let Some(e) = app.sound.error()
                    {
                        ui.label("");
                        ui.label(RichText::new(format!("No sound: {e}")).weak());
                        ui.end_row();
                    }
                });

            section(ui, "New battles");
            egui::Grid::new("battle-defaults")
                .num_columns(2)
                .min_col_width(LABELS)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Mode");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.classic, false, "Deterministic");
                        ui.radio_value(&mut s.classic, true, "Classic timing");
                    });
                    ui.end_row();
                    start_paused_row(ui, &mut s.start_paused);
                    ui.label("Rounds");
                    ui.add(egui::DragValue::new(&mut s.rounds).range(1..=10_000));
                    ui.end_row();
                    ui.label("Battlefield size");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.width).range(400..=5000));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut s.height).range(400..=5000));
                    });
                    ui.end_row();
                    rules_rows(ui, &mut s.rules);
                });

            section(ui, "Updates");
            ui.checkbox(
                &mut app.settings.check_updates,
                "Look for a newer release once a day",
            );
            ui.horizontal(|ui| {
                let busy = app.updater.busy();
                if ui
                    .add_enabled(!busy, egui::Button::new("Look now"))
                    .clicked()
                {
                    app.updater.look_now(ui.ctx());
                }
                ui.label(
                    RichText::new(format!("This is robocode-s {}.", env!("CARGO_PKG_VERSION")))
                        .weak(),
                );
            });
            ui.add(
                egui::Label::new(
                    RichText::new(crate::update::describe(app.updater.installation()))
                        .small()
                        .weak(),
                )
                .wrap(),
            );

            if let Some(e) = &app.settings_error {
                ui.add_space(12.0);
                ui.colored_label(
                    Color32::LIGHT_RED,
                    format!("Couldn't save the options: {e}"),
                );
            }
        });
        if self.settings != before {
            self.settings_changed(ctx, &before);
        } else if rescan {
            self.rescan(ctx);
        }
    }

    /// Saves the options and applies them: new robot folders are scanned, and new
    /// defaults go into the battle setup.
    pub(super) fn settings_changed(&mut self, ctx: &egui::Context, before: &Settings) {
        self.settings_unsaved = Some(ctx.input(|i| i.time));
        self.env.sandbox = self.settings.sandbox();
        if !self.settings.warm_hosts || self.settings.sandbox != before.sandbox {
            self.pool = None;
        }
        let (s, setup) = (&self.settings, &mut self.setup);
        if s.classic != before.classic {
            setup.classic = s.classic;
        }
        if s.rounds != before.rounds {
            setup.rounds = s.rounds;
        }
        if (s.width, s.height) != (before.width, before.height) {
            (setup.width, setup.height) = (s.width, s.height);
        }
        if s.rules != before.rules {
            setup.rules = s.rules;
        }
        if s.start_paused != before.start_paused {
            setup.paused = s.start_paused;
        }
        if s.robot_folders != before.robot_folders {
            self.rescan(ctx);
        }
    }
}

/// Start Battle's rows for the game: its type, rounds, battlefield, seed and mode, and the
/// `.battle` file buttons (Open sets `browse`, Save sets `save_battle`).
pub(super) fn game_rows(
    ui: &mut egui::Ui,
    s: &mut Setup,
    browse: &mut Option<Browse>,
    save_battle: &mut bool,
) {
    ui.label("Game type");
    ui.horizontal(|ui| {
        let mut custom = true;
        for game in &GAME_TYPES {
            let on = game.matches(s);
            custom &= !on;
            if ui
                .selectable_label(on, game.name)
                .on_hover_text(game.hint)
                .clicked()
            {
                game.apply(s);
            }
        }
        ui.add_enabled(false, egui::Button::selectable(custom, "Custom"))
            .on_disabled_hover_text("Rules of your own: change any below");
    });
    ui.end_row();
    ui.label("Rounds");
    ui.add(egui::DragValue::new(&mut s.rounds).range(1..=10_000));
    ui.end_row();
    ui.label("Battlefield size");
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut s.width).range(400..=5000));
        ui.label("×");
        ui.add(egui::DragValue::new(&mut s.height).range(400..=5000));
    });
    ui.end_row();
    ui.label("Seed");
    ui.add(
        egui::TextEdit::singleline(&mut s.seed)
            .hint_text("random")
            .desired_width(160.0),
    );
    ui.end_row();
    ui.label("Mode");
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.classic, false, "Deterministic");
        ui.radio_value(&mut s.classic, true, "Classic timing");
    });
    ui.end_row();
    ui.label("Battle file");
    ui.horizontal(|ui| {
        if ui
            .button("Open…")
            .on_hover_text("The robots and rules from a classic .battle file")
            .clicked()
        {
            *browse = Some(Browse::OpenBattle);
        }
        *save_battle = ui
            .button("Save…")
            .on_hover_text("The robots and rules as a classic .battle file")
            .clicked();
    });
    ui.end_row();
}

/// Start Battle's rows for the rules (see [`rules_rows`]), debug paint and the recording
/// (its folder button sets `browse`).
pub(super) fn rules_and_output_rows(ui: &mut egui::Ui, s: &mut Setup, browse: &mut Option<Browse>) {
    rules_rows(ui, &mut s.rules);
    ui.label("Debug paint");
    ui.checkbox(&mut s.paint, "On for every robot");
    ui.end_row();
    start_paused_row(ui, &mut s.paused);
    ui.label("Record to");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut s.record)
                .hint_text("a file (optional)")
                .desired_width((ui.available_width() - 40.0).clamp(120.0, 360.0)),
        );
        if browse_button(ui, "Choose the file to record to") {
            *browse = Some(Browse::Record);
        }
    });
    ui.end_row();
}

/// The row for starting battles paused (the setup's, and the options' default for it).
pub(super) fn start_paused_row(ui: &mut egui::Ui, paused: &mut bool) {
    ui.label("Start paused");
    ui.checkbox(paused, "Wait at the first turn")
        .on_hover_text("The battle opens paused: Resume, or One turn at a time.");
    ui.end_row();
}

/// The folder button after a file or folder field; true when clicked.
pub(super) fn browse_button(ui: &mut egui::Ui, hint: &str) -> bool {
    icons::button(ui, 22.0, hint, false, |p, c, color| {
        icons::folder(p, c, 7.0, color);
    })
    .clicked()
}

/// The options' sandbox row: what robots run in (ADR 0019). The sandboxes this platform
/// lacks are listed, disabled, with the reason.
pub(super) fn sandbox_row(ui: &mut egui::Ui, settings: &mut Settings) {
    let chosen = settings.sandbox();
    ui.label("Sandbox").on_hover_text(
        "What the robots run in: they are code from anyone. os is the system's own \
         sandbox (on Linux seccomp, and Landlock where the kernel has it; on Windows an \
         AppContainer and a Job Object): no network, and only the robot's files. bwrap is bubblewrap, if installed. docker is a \
         container of the Temurin JRE image, with no network (needs Docker; the image is pulled the first time); podman is the same \
         container in Podman. none is a plain \
         process, with all your rights.",
    );
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("sandbox")
            .selected_text(chosen.name())
            .show_ui(ui, |ui| {
                for sandbox in Sandbox::ALL {
                    let why = sandbox.unavailable();
                    let item = ui.add_enabled(
                        why.is_none(),
                        egui::Button::selectable(chosen == sandbox, sandbox.name()),
                    );
                    let item = match why {
                        Some(why) => item.on_disabled_hover_text(why),
                        None => item,
                    };
                    if item.clicked() {
                        settings.sandbox = sandbox.name().into();
                    }
                }
            });
        ui.label(RichText::new(chosen.report()).weak());
    });
    ui.end_row();
    if let Some(why) = settings
        .sandbox
        .parse::<Sandbox>()
        .err()
        .filter(|_| settings.sandbox != chosen.name())
    {
        ui.label("");
        ui.label(RichText::new(format!("{why}: {chosen} instead")).weak());
        ui.end_row();
    }
}

/// The rules' rows in a two-column grid: gun cooling rate, inactivity time, sentry border
/// and hiding enemy names.
pub(super) fn rules_rows(ui: &mut egui::Ui, rules: &mut Rules) {
    ui.label("Gun cooling rate");
    ui.add(
        egui::DragValue::new(&mut rules.gun_cooling_rate)
            .range(0.01..=1.0)
            .speed(0.01)
            .fixed_decimals(2),
    )
    .on_hover_text("Gun heat lost per turn (classic's default 0.1)");
    ui.end_row();
    ui.label("Inactivity time");
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut rules.inactivity_time).range(0..=1_000_000));
        ui.label("turns");
    })
    .response
    .on_hover_text("Turns without damage before the robots start losing energy");
    ui.end_row();
    ui.label("Sentry border");
    ui.add(egui::DragValue::new(&mut rules.sentry_border).range(50..=1000))
        .on_hover_text("How far into the battlefield a sentry robot's border reaches");
    ui.end_row();
    ui.label("Enemy names");
    ui.checkbox(&mut rules.hide_enemy_names, "Hide them from the robots");
    ui.end_row();
}

/// A typed folder: `~` at the start is the home directory.
pub(super) fn expand_home(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(home).join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(text),
    }
}

/// The robot jars and class folders in `paths`, e.g. `3 jars and 1 class folder`.
pub(super) fn sources(paths: &[PathBuf]) -> String {
    let folders = paths.iter().filter(|p| p.is_dir()).count();
    let jars = match paths.len() - folders {
        1 => "1 jar".to_string(),
        n => format!("{n} jars"),
    };
    match folders {
        0 => jars,
        1 => format!("{jars} and 1 class folder"),
        n => format!("{jars} and {n} class folders"),
    }
}
