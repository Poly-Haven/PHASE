use super::{AppState, ConflictChoice};
use crate::copy::plan::Action;

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let Some(pc) = state.pending_conflict.as_ref() else { return; };
    let slug = pc.key.slug.clone();
    let conflicts: Vec<(String, &'static str)> = pc.plan.files.iter()
        .filter_map(|f| match f.action {
            Action::Conflict { dest_newer: true }  => Some((f.rel_path.to_string_lossy().to_string(), "Newer at destination")),
            Action::Conflict { dest_newer: false } => Some((f.rel_path.to_string_lossy().to_string(), "Newer at source")),
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
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (path, note) in &conflicts {
                    ui.horizontal(|ui| {
                        ui.monospace(path);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(*note);
                        });
                    });
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Overwrite All").clicked() { choice = Some(ConflictChoice::OverwriteAll); }
                if ui.button("Copy Only New").clicked() { choice = Some(ConflictChoice::CopyOnlyNew); }
                if ui.button("Cancel").clicked()        { choice = Some(ConflictChoice::Cancel); }
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

pub fn token_prompt(_state: &mut AppState, _ctx: &egui::Context) {
    // Filled in by Task 9.
}
