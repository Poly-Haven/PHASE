use super::{AppState, AssetListState, AssetType};

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);

        for t in [AssetType::Hdris, AssetType::Textures] {
            let selected = state.current_type == t;
            if ui.selectable_label(selected, t.label()).clicked() && !selected {
                // Save the current filter before switching.
                let old_label = state.current_type.label().to_string();
                state
                    .config
                    .last_filters
                    .insert(old_label, state.author_filter.clone());
                state.current_type = t;
                // Restore the filter for the new tab.
                state.author_filter = state
                    .config
                    .last_filters
                    .get(t.label())
                    .cloned()
                    .unwrap_or_default();
                state.config.last_tab = t.label().to_string();
                let _ = crate::config::save(&state.config);
            }
        }

        ui.separator();

        let authors = current_authors(state);
        let display = if state.author_filter.is_empty() {
            "All authors".to_string()
        } else {
            state.author_filter.clone()
        };
        let filter_before = state.author_filter.clone();
        egui::ComboBox::from_id_source("author_filter")
            .selected_text(display)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.author_filter, String::new(), "All authors");
                for a in authors {
                    ui.selectable_value(&mut state.author_filter, a.clone(), a);
                }
            });
        if state.author_filter != filter_before {
            let label = state.current_type.label().to_string();
            state
                .config
                .last_filters
                .insert(label, state.author_filter.clone());
            let _ = crate::config::save(&state.config);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let is_loading = state.refreshing.contains(&state.current_type);
            let refresh_label = if is_loading {
                "Loading…"
            } else {
                "↻ Refresh"
            };
            if ui
                .add_enabled(!is_loading, egui::Button::new(refresh_label))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                state.refresh(state.current_type);
            }
            ui.separator();
            ui.label(env!("CARGO_PKG_VERSION"));
        });
    });
}

fn current_authors(state: &AppState) -> Vec<String> {
    let Some(AssetListState::Loaded(list)) = state.assets_by_type.get(&state.current_type) else {
        return Vec::new();
    };
    author_filter_options(list.iter().map(|a| a.author.as_str()))
}

fn author_filter_options<'a>(authors: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    super::authors::filter_options(authors)
}

#[cfg(test)]
mod tests {
    #[test]
    fn author_filter_options_split_multi_author_combinations_into_people() {
        let options = super::author_filter_options(["Alice, Bob", "Bob, Carol", "Alice", ""]);

        assert_eq!(options, vec!["Alice", "Bob", "Carol"]);
    }
}
