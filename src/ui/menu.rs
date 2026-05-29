use super::{AppState, AssetListState, AssetType};

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);

        for t in [AssetType::Hdris, AssetType::Textures] {
            let selected = state.current_type == t;
            if ui.selectable_label(selected, t.label()).clicked() && !selected {
                state.current_type = t;
                state.author_filter.clear();
            }
        }

        ui.separator();

        let authors = current_authors(state);
        let display = if state.author_filter.is_empty() {
            "All authors".to_string()
        } else {
            state.author_filter.clone()
        };
        egui::ComboBox::from_id_source("author_filter")
            .selected_text(display)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.author_filter, String::new(), "All authors");
                for a in authors {
                    ui.selectable_value(&mut state.author_filter, a.clone(), a);
                }
            });

        ui.separator();

        let is_loading = state.refreshing.contains(&state.current_type);
        let refresh_label = if is_loading { "Loading…" } else { "↻ Refresh" };
        if ui.add_enabled(!is_loading, egui::Button::new(refresh_label)).clicked() {
            state.refresh(state.current_type);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("PHASE {}", env!("CARGO_PKG_VERSION")));
        });
    });
}

fn current_authors(state: &AppState) -> Vec<String> {
    let Some(AssetListState::Loaded(list)) = state.assets_by_type.get(&state.current_type) else {
        return Vec::new();
    };
    let set: std::collections::BTreeSet<String> = list.iter()
        .map(|a| a.author.clone())
        .filter(|s| !s.is_empty())
        .collect();
    set.into_iter().collect()
}
