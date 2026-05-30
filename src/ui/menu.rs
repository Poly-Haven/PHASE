use super::{AppState, AssetListState, AssetType};
use crate::notion::StatusGroup;

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

        let status_options: Vec<_> = StatusGroup::all()
            .iter()
            .map(|g| super::group_selector::OptionItem {
                value: *g,
                label: g.label(),
                selected_bg: Some(super::status_groups::selected_color(*g)),
            })
            .collect();
        let response = super::group_selector::draw(
            ui,
            "status_group_selector",
            &status_options,
            &state.selected_status_groups,
        );
        if let Some(clicked) = response.clicked {
            state.selected_status_groups = super::status_groups::select(
                state.selected_status_groups.clone(),
                clicked,
                response.additive,
            );
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
            let gear_tex = super::gear_texture(ui.ctx());
            let gear_resp = ui.add(
                egui::Image::new(egui::load::SizedTexture::new(
                    gear_tex.id(),
                    egui::vec2(16.0, 16.0),
                ))
                .tint(super::colors::TEXT_PRIMARY)
                .sense(egui::Sense::click()),
            );
            if gear_resp
                .on_hover_text("Settings")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                state.open_settings();
            }
            ui.add_space(6.0);

            let is_loading = state
                .selected_types
                .iter()
                .any(|t| state.refreshing.contains(t));
            if is_loading {
                super::loading_indicator::draw_button(ui);
            } else {
                if ui
                    .button("↻ Refresh")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    for t in state.selected_types.clone() {
                        state.refresh(t);
                    }
                }
            }
        });
    });
}

fn current_authors(state: &AppState) -> Vec<String> {
    let status_filter = &state.selected_status_groups;
    let mut authors = Vec::new();
    for t in &state.selected_types {
        if let Some(AssetListState::Loaded(list)) = state.assets_by_type.get(t) {
            authors.extend(
                list.assets
                    .iter()
                    .filter(|a| {
                        a.status
                            .as_ref()
                            .map(|s| status_filter.contains(&s.group))
                            .unwrap_or(false)
                    })
                    .map(|a| a.author.as_str()),
            );
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
    use crate::notion::{Asset, AssetStatus, StatusGroup};

    fn asset(author: &str, group: StatusGroup) -> Asset {
        Asset {
            page_id: String::new(),
            slug: "test".into(),
            author: author.to_string(),
            url: String::new(),
            status: Some(AssetStatus {
                id: String::new(),
                name: String::new(),
                color: String::new(),
                group,
            }),
        }
    }

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

    #[test]
    fn author_list_excludes_authors_whose_assets_dont_match_status_filter() {
        let assets = [
            asset("Greg", StatusGroup::Complete),
            asset("Didier", StatusGroup::InProgress),
        ];
        let status_filter = &[StatusGroup::InProgress];

        let authors: Vec<_> = assets
            .iter()
            .filter(|a| {
                a.status
                    .as_ref()
                    .map(|s| status_filter.contains(&s.group))
                    .unwrap_or(false)
            })
            .map(|a| a.author.as_str())
            .collect();

        let options = super::author_filter_options(authors);
        assert_eq!(options, vec!["Didier"]);
        assert!(!options.iter().any(|o| o == "Greg"));
    }
}
