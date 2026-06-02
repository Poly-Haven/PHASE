use super::{AppState, ConflictChoice};
use crate::copy::plan::Action;

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let Some(pc) = state.pending_conflict.as_ref() else {
        return;
    };
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
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("{} file(s) in conflict:", conflicts.len()));
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
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
            ui.add_space(8.0);
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
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Log in with Poly Haven to load and update PHASE asset statuses.");
            ui.add_space(8.0);
            ui.label("Tokens will be saved to:");
            ui.monospace(
                crate::config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
            ui.add_space(8.0);
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
            ui.add_space(8.0);
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
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Local root path");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.settings_local_root_input)
                        .desired_width(420.0),
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

            ui.add_space(8.0);
            ui.checkbox(
                &mut state.settings_skip_pull_raw_tif_if_many_work_tifs,
                "Skip pulling RAW and TIF files if >30 TIFs exist in /work",
            );

            ui.add_space(12.0);
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
        state.config.skip_pull_raw_tif_if_many_work_tifs =
            state.settings_skip_pull_raw_tif_if_many_work_tifs;
        if let Err(e) = crate::config::save(&state.config) {
            state.error_banner = Some(format!("Failed to save config: {e}"));
        } else {
            state.settings_open = false;
        }
    } else if close {
        state.settings_open = false;
    }
}
