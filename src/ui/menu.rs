use super::{AppState, AssetListState, AssetType};

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(30.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);

        let options: Vec<_> = AssetType::all()
            .iter()
            .map(|t| super::group_selector::OptionItem {
                value: *t,
                label: t.label(),
                selected_bg: Some(t.selected_color()),
            })
            .collect();
        let response =
            super::group_selector::draw(ui, "asset_type_selector", &options, &state.selected_types);
        if let Some(clicked) = response.clicked {
            state.selected_types = super::asset_types::select(
                state.selected_types.clone(),
                clicked,
                response.additive,
            );
            state.current_type = state.selected_types.first().copied().unwrap_or(clicked);
            state.config.last_tab = state.current_type.label().to_string();
            state.config.last_asset_types = super::asset_types::labels(&state.selected_types);
            let _ = crate::config::save(&state.config);
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
            state.config.last_author_filter = state.author_filter.clone();
            let _ = crate::config::save(&state.config);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let is_loading = state
                .selected_types
                .iter()
                .any(|t| state.refreshing.contains(t));
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
                for t in state.selected_types.clone() {
                    state.refresh(t);
                }
            }
            ui.separator();
            ui.label(env!("CARGO_PKG_VERSION"));
        });
    });
}

fn current_authors(state: &AppState) -> Vec<String> {
    let mut authors = Vec::new();
    for t in &state.selected_types {
        if let Some(AssetListState::Loaded(list)) = state.assets_by_type.get(t) {
            authors.extend(list.iter().map(|a| a.author.as_str()));
        }
    }
    author_filter_options_with_current(authors, &state.author_filter)
}

fn author_filter_options<'a>(authors: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    super::authors::filter_options(authors)
}

fn author_filter_options_with_current<'a>(
    authors: impl IntoIterator<Item = &'a str>,
    current: &str,
) -> Vec<String> {
    let mut options = author_filter_options(authors);
    if !current.is_empty() && !options.iter().any(|option| option == current) {
        options.push(current.to_string());
        options.sort();
    }
    options
}

#[cfg(test)]
mod tests {
    #[test]
    fn author_filter_options_split_multi_author_combinations_into_people() {
        let options = super::author_filter_options(["Alice, Bob", "Bob, Carol", "Alice", ""]);

        assert_eq!(options, vec!["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn author_filter_options_include_current_selection_even_without_matching_assets() {
        let options = super::author_filter_options_with_current(["Alice, Bob"], "Carol");

        assert_eq!(options, vec!["Alice", "Bob", "Carol"]);
    }
}
