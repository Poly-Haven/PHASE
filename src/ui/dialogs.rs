use super::{colors, layout, AppState, ConflictChoice};
use crate::copy::plan::Action;
use crate::copy::plan::Direction;

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    if let Some(pc) = state.pending_conflict.as_ref() {
        let slug = pc.key.slug.clone();
        let conflicts: Vec<(String, &'static str)> = pc
            .plan
            .files
            .iter()
            .filter_map(|f| match f.action {
                Action::Conflict { dest_newer: true } => Some((
                    f.rel_path.to_string_lossy().to_string(),
                    "Newer at destination",
                )),
                Action::Conflict { dest_newer: false } => {
                    Some((f.rel_path.to_string_lossy().to_string(), "Newer at source"))
                }
                _ => None,
            })
            .collect();

        let mut choice: Option<ConflictChoice> = None;

        egui::Window::new(format!("Conflicts — {slug}"))
            .collapsible(false)
            .resizable(true)
            .default_width(layout::CONFLICT_DIALOG_WIDTH)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{} file(s) in conflict:", conflicts.len()));
                ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);
                egui::ScrollArea::vertical()
                    .max_height(layout::CONFLICT_DIALOG_SCROLL_HEIGHT)
                    .show(ui, |ui| {
                        for (path, note) in &conflicts {
                            ui.horizontal(|ui| {
                                ui.monospace(path);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.weak(*note);
                                    },
                                );
                            });
                        }
                    });
                ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
                ui.horizontal(|ui| {
                    if ui.button("Overwrite All").clicked() {
                        choice = Some(ConflictChoice::OverwriteAll);
                    }
                    if ui.button("Copy Only New").clicked() {
                        choice = Some(ConflictChoice::CopyOnlyNew);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(ConflictChoice::Cancel);
                    }
                });
            });

        if let Some(c) = choice {
            if matches!(c, ConflictChoice::Cancel) {
                state.pending_conflict = None;
            } else {
                super::execute_after_conflict(state, c);
            }
        }
    }

    super::scripts::draw_output_dialog(state, ctx);
    transfer_file_list(state, ctx);
}

pub fn token_prompt(state: &mut AppState, ctx: &egui::Context) {
    if !state.token_prompt_open {
        return;
    }
    if state.auth_rx.is_none() && state.auth_login.is_none() {
        state.start_auth_login();
    }
    let mut retry = false;
    let mut close = false;
    egui::Window::new("PHASE login required")
        .collapsible(false)
        .resizable(true)
        .default_width(layout::TOKEN_PROMPT_WIDTH)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Log in with Poly Haven to load and update PHASE asset statuses.");
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            ui.label("Tokens will be saved to:");
            ui.monospace(
                crate::config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            if let Some(login) = &state.auth_login {
                ui.label("A browser window should open. If it does not, visit:");
                ui.hyperlink(&login.auth_url);
                ui.label("After login, Auth0 will redirect back to:");
                ui.monospace(&login.redirect_uri);
                ui.label("PHASE will continue automatically when the browser login finishes.");
            } else if state.auth_rx.is_some() {
                ui.label("Starting browser login...");
            } else {
                ui.label("Login is not currently running.");
            }
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            ui.horizontal(|ui| {
                if ui.button("Restart login").clicked() {
                    retry = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if retry {
        if let Some(login) = &state.auth_login {
            if let Err(err) = open::that(&login.auth_url) {
                state.error_banner = Some(format!("Failed to open login page: {err}"));
            }
        } else {
            state.auth_rx = None;
            state.auth_login = None;
            state.start_auth_login();
        }
    } else if close {
        state.token_prompt_open = false;
    }
}

pub fn settings(state: &mut AppState, ctx: &egui::Context) {
    if !state.settings_open {
        return;
    }

    let mut save = false;
    let mut close = false;
    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(false)
        .default_width(layout::SETTINGS_DIALOG_WIDTH)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Local root path");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.settings_local_root_input)
                        .desired_width(layout::SETTINGS_LOCAL_ROOT_WIDTH),
                );
                if ui
                    .button("Select...")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_directory(&state.config.local_root)
                        .pick_folder()
                    {
                        state.settings_local_root_input = path.display().to_string();
                    }
                }
            });

            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            ui.checkbox(
                &mut state.settings_open_notion_links_in_desktop_app,
                "Open Notion links in the desktop app",
            );

            ui.add_space(layout::DIALOG_SECTION_SPACING_LARGE);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if save {
        let local_root = state.settings_local_root_input.trim();
        if local_root.is_empty() {
            state.error_banner = Some("Local root path cannot be empty".into());
            return;
        }
        state.config.local_root = std::path::PathBuf::from(local_root);
        state.config.open_notion_links_in_desktop_app =
            state.settings_open_notion_links_in_desktop_app;
        if let Err(e) = crate::config::save(&state.config) {
            state.error_banner = Some(format!("Failed to save config: {e}"));
        } else {
            state.settings_open = false;
        }
    } else if close {
        state.settings_open = false;
    }
}

pub fn transfer_file_list(state: &mut AppState, ctx: &egui::Context) {
    let Some(dialog) = state.transfer_file_list_dialog.as_ref() else {
        return;
    };

    let mut close = false;
    let mut reload = false;
    let mut ignore_raws_tiffs = dialog.ignore_raws_tiffs;
    let plan = dialog.plan.clone();
    let direction = dialog.direction;
    let key_slug = dialog.key.slug.clone();

    egui::Window::new(format!("File list — {key_slug}"))
        .collapsible(false)
        .resizable(true)
        .default_width(layout::TRANSFER_FILE_LIST_DIALOG_WIDTH)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            match direction {
                Direction::Push => {
                    ui.label("This list uses the default push behavior: all files.");
                }
                Direction::Pull => {
                    ui.label("This list uses the default pull behavior.");
                    if ui
                        .checkbox(&mut ignore_raws_tiffs, "Ignore raws/tiffs")
                        .changed()
                    {
                        reload = true;
                    }
                }
            }

            if let Some(err) = &dialog.error {
                ui.colored_label(colors::ERROR_BANNER, err);
            }
            if dialog.loading {
                ui.label("Loading file list...");
            }

            if let Some(plan) = plan.as_ref() {
                let rows = super::transfer_file_display_rows(plan);
                let width = transfer_file_list_width(ui, &rows);
                ui.set_min_width(width);
                ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);

                if rows.is_empty() {
                    ui.label("No files will be transferred.");
                } else {
                    ui.label(format!("{} file(s) will be transferred:", rows.len()));
                    ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);
                    egui::ScrollArea::both()
                        .max_height(layout::TRANSFER_FILE_LIST_SCROLL_HEIGHT)
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            for row in rows {
                                ui.horizontal(|ui| {
                                    ui.colored_label(row.color, &row.path);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.colored_label(row.color, row.reason);
                                        },
                                    );
                                });
                            }
                        });
                }
            }

            ui.add_space(layout::DIALOG_SECTION_SPACING_LARGE);
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });

    if reload {
        if let Some(dialog) = state.transfer_file_list_dialog.as_mut() {
            dialog.ignore_raws_tiffs = ignore_raws_tiffs;
        }
        state.reload_transfer_file_list();
    }
    if close {
        state.transfer_file_list_dialog = None;
    }
}

fn transfer_file_list_width(ui: &egui::Ui, rows: &[super::TransferFileDisplayRow]) -> f32 {
    let path_font = egui::TextStyle::Monospace.resolve(ui.style());
    let reason_font = egui::TextStyle::Body.resolve(ui.style());
    let max_path = rows
        .iter()
        .map(|row| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(row.path.clone(), path_font.clone(), egui::Color32::WHITE)
                    .rect
                    .width()
            })
        })
        .fold(0.0, f32::max);
    let max_reason = rows
        .iter()
        .map(|row| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(
                        row.reason.to_string(),
                        reason_font.clone(),
                        egui::Color32::WHITE,
                    )
                    .rect
                    .width()
            })
        })
        .fold(0.0, f32::max);
    (max_path + max_reason + layout::STATUS_OPTION_WIDTH_PADDING * 2.0)
        .max(layout::TRANSFER_FILE_LIST_DIALOG_WIDTH)
}
