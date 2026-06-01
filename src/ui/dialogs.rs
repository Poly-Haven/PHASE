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
    let mut save = false;
    let mut close = false;
    egui::Window::new("Notion token required")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Paste your Notion integration token. It will be saved to");
            ui.monospace(
                crate::config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.token_input)
                    .password(true)
                    .desired_width(400.0),
            );
            ui.add_space(8.0);
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
        state.config.notion_token = state.token_input.trim().to_string();
        if let Err(e) = crate::config::save(&state.config) {
            state.error_banner = Some(format!("Failed to save config: {e}"));
        }
        state.token_prompt_open = false;
        state.refresh(super::AssetType::Hdris);
        state.refresh(super::AssetType::Textures);
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
