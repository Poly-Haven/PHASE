use super::colors;
use super::{AppState, AssetListState, RowKey};
use crate::copy::plan::Direction;
use crate::notion::{Asset, AssetStatus, StatusGroup, StatusOption};

/// Severity of a row-level message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Warning,
    Error,
    Question,
}

/// A single message attached to an asset row.
#[derive(Clone)]
pub struct RowMsg {
    pub kind: MsgKind,
    pub text: String,
    pub link: Option<String>,
}

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    let filter = state.author_filter.clone();
    let status_groups = state.selected_status_groups.clone();
    let selected_types = state.selected_types.clone();
    let mut rows = Vec::new();
    let mut has_loaded_list = false;
    let mut loading_count = 0;
    let mut errors = Vec::new();

    for t in selected_types {
        match state.assets_by_type.get(&t) {
            None | Some(AssetListState::Loading) => {
                loading_count += 1;
            }
            Some(AssetListState::Error(msg)) => {
                errors.push(format!("{}: {msg}", t.label()));
            }
            Some(AssetListState::Loaded(list)) => {
                has_loaded_list = true;
                let prod_root = state.prod_root_for(t);
                rows.extend(
                    list.assets
                        .iter()
                        .filter(|a| author_matches_filter(&a.author, &filter))
                        .filter(|a| status_matches_filter(&a.status, &status_groups))
                        .map(|a| RowView::from_asset(t, a, &prod_root, &state.published_assets)),
                );
            }
        }
    }

    if !errors.is_empty() {
        ui.colored_label(colors::ERROR_BANNER, errors.join(" · "));
    }
    if !has_loaded_list && loading_count > 0 {
        ui.label("Loading…");
        return;
    }

    rows.sort_by(|a, b| {
        b.exists_on_prod
            .cmp(&a.exists_on_prod)
            .then_with(|| a.asset_type.order().cmp(&b.asset_type.order()))
            .then_with(|| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()))
    });

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for row in rows {
                let key = RowKey {
                    asset_type: row.asset_type,
                    slug: row.slug.clone(),
                };
                draw_row(state, ui, &key, &row);
            }
        });
}

struct RowView {
    asset_type: super::AssetType,
    slug: String,
    author: String,
    url: String,
    page_id: String,
    status: Option<AssetStatus>,
    exists_on_prod: bool,
    messages: Vec<RowMsg>,
}

impl RowView {
    fn from_asset(
        asset_type: super::AssetType,
        a: &Asset,
        prod_root: &std::path::Path,
        published_assets: &crate::polyhaven::PublishedAssets,
    ) -> Self {
        let exists_on_prod = prod_root.join(&a.slug).is_dir();
        let mut messages = Vec::new();
        if let Some(text) = crate::slug::message(&a.slug) {
            messages.push(RowMsg {
                kind: MsgKind::Error,
                text,
                link: None,
            });
        }
        if !exists_on_prod {
            messages.push(RowMsg {
                kind: MsgKind::Warning,
                text: "Prod folder missing".into(),
                link: None,
            });
        }
        if should_warn_published_slug(published_assets, &a.slug, &a.status) {
            messages.push(RowMsg {
                kind: MsgKind::Warning,
                text: "Published asset with this slug found".into(),
                link: Some(format!("https://polyhaven.com/a/{}", a.slug)),
            });
        }
        Self {
            asset_type,
            slug: a.slug.clone(),
            author: a.author.clone(),
            url: a.url.clone(),
            page_id: a.page_id.clone(),
            status: a.status.clone(),
            exists_on_prod,
            messages,
        }
    }
}

fn draw_row(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let row_height = 28.0;
    let avail = ui.available_rect_before_wrap();
    let row_rect = egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), row_height));

    ui.painter()
        .rect_filled(row_rect, 2.0, colors::ROW_BACKGROUND);
    if let Some(job) = state.jobs.get(key) {
        let f = job.progress.fraction().clamp(0.0, 1.0);
        let mut fill = row_rect;
        fill.set_width(avail.width() * f);
        ui.painter()
            .rect_filled(fill, 2.0, colors::colored_background(colors::PROGRESS_BAR));
    }

    let prod_folder = state.prod_root_for(key.asset_type).join(&key.slug);
    let local_folder = state.local_root_for(key.asset_type).join(&key.slug);
    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let row_response = ui.interact(
        row_rect,
        ui.id().with(("row-context", key.asset_type, &key.slug)),
        egui::Sense::click(),
    );
    row_response.context_menu(|ui| draw_context_menu(ui, &local_folder, &prod_folder, &row.url));

    ui.allocate_ui_at_rect(row_rect, |ui| {
        ui.horizontal_centered(|ui| {
            ui.add_space(8.0);
            let text_color = if row.exists_on_prod {
                colors::TEXT_PRIMARY
            } else {
                colors::TEXT_DISABLED
            };

            // Slug — pre-layout to get same-frame hover detection.
            let font_id = egui::TextStyle::Body.resolve(ui.style());
            let galley =
                ui.fonts(|f| f.layout_no_wrap(row.slug.clone(), font_id, egui::Color32::WHITE));
            let slug_size = galley.rect.size();
            let slug_start = egui::pos2(ui.cursor().min.x, row_rect.center().y - slug_size.y / 2.0);
            let slug_rect = egui::Rect::from_min_size(slug_start, slug_size);
            let is_slug_hovered = ui.rect_contains_pointer(slug_rect);
            let slug_color = if is_slug_hovered {
                colors::HOVER
            } else {
                text_color
            };
            let slug_label = egui::Label::new(egui::RichText::new(&row.slug).color(slug_color))
                .sense(egui::Sense::click());
            let slug_resp = ui.add(slug_label);
            let slug_resp = slug_resp
                .on_hover_text(if row.exists_on_prod {
                    "Open in Explorer"
                } else {
                    "Create Prod folder"
                })
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if slug_resp.clicked() {
                if row.exists_on_prod {
                    let _ = open::that(&prod_folder);
                } else if crate::slug::is_valid(&row.slug) {
                    state.pending_prod_folder_create = Some(key.clone());
                } else {
                    state.error_banner =
                        Some("Cannot create Prod folder: slug has invalid characters".into());
                }
            }

            // Notion button — sits immediately to the right of the slug.
            let notion_size = egui::vec2(14.0, 14.0);
            let notion_tex = super::notion_logo_texture(ui.ctx());
            let (notion_rect, notion_resp) =
                ui.allocate_exact_size(notion_size, egui::Sense::click());
            if ui.is_rect_visible(notion_rect) {
                let base_tint = text_color.linear_multiply(0.6);
                let tint = if notion_resp.hovered() {
                    colors::HOVER
                } else {
                    base_tint
                };
                ui.painter()
                    .image(notion_tex.id(), notion_rect, uv_full, tint);
            }
            if notion_resp
                .on_hover_text("Open in Notion")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                let _ = open::that(&row.url);
            }

            ui.add_space(16.0);
            ui.colored_label(text_color.linear_multiply(0.8), &row.author);
            ui.add_space(8.0);
            draw_status_pill(state, ui, key, row);

            // Row messages (icons + text, left-to-right after author).
            if let Some(toast) = state.row_toasts.get(key) {
                draw_toast(ui, toast);
            }

            for msg in &row.messages {
                ui.add_space(8.0);
                let (tex, color) = match msg.kind {
                    MsgKind::Info => (super::info_icon_texture(ui.ctx()), colors::MSG_INFO),
                    MsgKind::Warning => (super::warn_icon_texture(ui.ctx()), colors::MSG_WARNING),
                    MsgKind::Error => (super::error_icon_texture(ui.ctx()), colors::MSG_ERROR),
                    MsgKind::Question => {
                        (super::question_icon_texture(ui.ctx()), colors::MSG_QUESTION)
                    }
                };
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(14.0, 14.0),
                    ))
                    .tint(color),
                );
                ui.add_space(2.0);
                ui.colored_label(color, &msg.text);
                if let Some(link) = &msg.link {
                    ui.add_space(2.0);
                    let tex = super::external_link_texture(ui.ctx());
                    let resp = ui.add(
                        egui::Image::new(egui::load::SizedTexture::new(
                            tex.id(),
                            egui::vec2(12.0, 12.0),
                        ))
                        .tint(color)
                        .sense(egui::Sense::click()),
                    );
                    if resp
                        .on_hover_text("Open on polyhaven.com")
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        let _ = open::that(link);
                    }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw_row_actions(state, ui, key, row);
            });
        });
    });

    ui.advance_cursor_after_rect(row_rect);
    ui.separator();
}

fn draw_row_actions(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let text_color = if row.exists_on_prod {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_DISABLED
    };
    let icon_size = egui::vec2(18.0, 18.0);
    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

    // Allocate space first so we know the rect, then paint with same-frame hover tint.
    let icon_button = |ui: &mut egui::Ui,
                       tex: &egui::TextureHandle,
                       enabled: bool,
                       tooltip: &str|
     -> egui::Response {
        let sense = if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(icon_size, sense);
        if ui.is_rect_visible(rect) {
            let tint = if resp.hovered() && enabled {
                colors::HOVER
            } else {
                text_color
            };
            ui.painter().image(tex.id(), rect, uv_full, tint);
        }
        let cursor = if enabled {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::NotAllowed
        };
        resp.on_hover_text(tooltip).on_hover_cursor(cursor)
    };

    if state.jobs.contains_key(key) {
        if ui
            .button("✕")
            .on_hover_text("Cancel")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            if let Some(job) = state.jobs.get(key) {
                job.progress
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let job = state.jobs.get(key).unwrap();
        let done = job
            .progress
            .bytes_done
            .load(std::sync::atomic::Ordering::Relaxed);
        let tot = job
            .progress
            .bytes_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let label = match job.direction {
            Direction::Pull => "Pulling from Prod",
            Direction::Push => "Pushing from Local",
        };
        ui.label(format!(
            "{label}  ·  {} / {}",
            fmt_bytes(done),
            fmt_bytes(tot)
        ));
        return;
    }

    // Padding on the far right of the action strip (RTL: first space = rightmost).
    ui.add_space(8.0);
    draw_row_context_button(state, ui, key, row);
    ui.add_space(6.0);
    let enabled = row.exists_on_prod;
    let push_tex = super::push_icon_texture(ui.ctx());
    if icon_button(ui, &push_tex, enabled, "Push to Prod").clicked() {
        super::start_job(state, key, Direction::Push);
    }
    let pull_tex = super::pull_icon_texture(ui.ctx());
    if icon_button(ui, &pull_tex, enabled, "Pull from Prod").clicked() {
        super::start_job(state, key, Direction::Pull);
    }
}

fn draw_status_pill(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let Some(status) = &row.status else {
        ui.colored_label(colors::TEXT_DISABLED, "No status");
        return;
    };

    let is_updating = state.status_updates.contains_key(key);
    let options = status_options_for(state, key.asset_type);
    let popup_id = ui.make_persistent_id(("status_popup", &key.asset_type, &key.slug));
    let response = status_pill_button(ui, status, is_updating);

    if response.clicked() && !is_updating {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }

    let popup_width = status_dropdown_width(ui, &options);
    egui::popup::popup_below_widget(ui, popup_id, &response, |ui| {
        ui.set_min_width(popup_width);
        for group in StatusGroup::all() {
            let group_options: Vec<_> = options
                .iter()
                .filter(|option| option.group == *group)
                .collect();
            if group_options.is_empty() {
                continue;
            }
            ui.strong(format!("{}:", group.label()));
            for option in group_options {
                let resp = colored_status_option(
                    ui,
                    &option.name,
                    notion_color(&option.color),
                    popup_width,
                );
                if resp.clicked() {
                    super::start_status_update(state, key, &row.page_id, option.clone());
                    ui.close_menu();
                }
            }

            ui.add_space(4.0);
        }
    });
}

fn draw_context_menu(
    ui: &mut egui::Ui,
    local_folder: &std::path::Path,
    prod_folder: &std::path::Path,
    notion_url: &str,
) {
    if ui.button("Open local folder").clicked() {
        let _ = open::that(local_folder);
        ui.close_menu();
    }
    if ui.button("Open prod folder").clicked() {
        let _ = open::that(prod_folder);
        ui.close_menu();
    }
    if ui.button("Open in Notion").clicked() {
        let _ = open::that(notion_url);
        ui.close_menu();
    }
}

fn draw_row_context_button(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let icon_size = egui::vec2(18.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(icon_size, egui::Sense::click());
    let tex = super::chevron_down_texture(ui.ctx());
    let tint = if response.hovered() {
        colors::HOVER
    } else if row.exists_on_prod {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_DISABLED
    };
    ui.painter().image(
        tex.id(),
        rect.shrink(3.0),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        tint,
    );
    let response = response
        .on_hover_text("More actions")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let popup_id = ui.make_persistent_id(("row_context_popup", &key.asset_type, &key.slug));
    if response.clicked() {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }
    egui::popup::popup_below_widget(ui, popup_id, &response, |ui| {
        ui.set_min_width(140.0);
        let local_folder = state.local_root_for(key.asset_type).join(&key.slug);
        let prod_folder = state.prod_root_for(key.asset_type).join(&key.slug);
        draw_context_menu(ui, &local_folder, &prod_folder, &row.url);
    });
}

fn draw_toast(ui: &mut egui::Ui, toast: &super::RowToast) {
    let age = toast.created_at.elapsed().as_secs_f32();
    let alpha = if age > 3.0 {
        (1.0 - (age - 3.0) / 2.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let color = colors::MSG_QUESTION.linear_multiply(alpha);
    ui.add_space(8.0);
    let tex = super::check_texture(ui.ctx());
    ui.add(
        egui::Image::new(egui::load::SizedTexture::new(
            tex.id(),
            egui::vec2(14.0, 14.0),
        ))
        .tint(color),
    );
    ui.add_space(2.0);
    ui.colored_label(color, &toast.text);
}

fn status_dropdown_width(ui: &egui::Ui, options: &[StatusOption]) -> f32 {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let max_label = options
        .iter()
        .map(|option| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(option.name.clone(), font_id.clone(), egui::Color32::WHITE)
                    .rect
                    .width()
            })
        })
        .fold(0.0, f32::max);
    (max_label + 24.0).max(120.0)
}

fn colored_status_option(
    ui: &mut egui::Ui,
    label: &str,
    bg: egui::Color32,
    width: f32,
) -> egui::Response {
    let height = 20.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let bg = colors::colored_background(bg);
    let fill = if response.hovered() {
        bg.linear_multiply(1.25)
    } else {
        bg
    };
    ui.painter()
        .rect_filled(rect.shrink2(egui::vec2(1.0, 1.0)), 4.0, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        egui::Color32::WHITE,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn status_pill_button(
    ui: &mut egui::Ui,
    status: &AssetStatus,
    is_updating: bool,
) -> egui::Response {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let text_width = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(status.name.clone(), font_id.clone(), egui::Color32::WHITE)
            .rect
            .width()
    });
    let icon_size = egui::vec2(10.0, 10.0);
    let padding = egui::vec2(8.0, 3.0);
    let height = 18.0;
    let width = text_width + icon_size.x + padding.x * 3.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let bg = colors::colored_background(notion_color(&status.color));
    ui.painter().rect_filled(rect, height / 2.0, bg);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        height / 2.0,
        egui::Stroke::new(1.0, notion_color(&status.color)),
    );
    ui.painter().text(
        egui::pos2(rect.left() + padding.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &status.name,
        font_id,
        egui::Color32::WHITE,
    );

    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.right() - padding.x - icon_size.x / 2.0,
            rect.center().y,
        ),
        icon_size,
    );
    if is_updating {
        super::loading_indicator::draw_image_at(ui, icon_rect, egui::Color32::WHITE);
    } else {
        let tex = super::chevron_down_texture(ui.ctx());
        ui.painter().image(
            tex.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn status_options_for(state: &AppState, asset_type: super::AssetType) -> Vec<StatusOption> {
    match state.assets_by_type.get(&asset_type) {
        Some(AssetListState::Loaded(list)) => list.statuses.clone(),
        _ => Vec::new(),
    }
}

fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = b as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{b:.0} B")
    }
}

fn author_matches_filter(author: &str, filter: &str) -> bool {
    super::authors::contains(author, filter)
}

fn status_matches_filter(status: &Option<AssetStatus>, selected: &[StatusGroup]) -> bool {
    status
        .as_ref()
        .map(|status| selected.contains(&status.group))
        .unwrap_or(false)
}

fn should_warn_published_slug(
    published_assets: &crate::polyhaven::PublishedAssets,
    slug: &str,
    status: &Option<AssetStatus>,
) -> bool {
    published_assets.slugs.contains(slug)
        && status
            .as_ref()
            .map(|status| status.group != StatusGroup::Complete)
            .unwrap_or(true)
}

fn notion_color(color: &str) -> egui::Color32 {
    match color {
        "red" => egui::Color32::from_rgb(180, 68, 68),
        "orange" => egui::Color32::from_rgb(190, 110, 45),
        "yellow" => egui::Color32::from_rgb(170, 135, 40),
        "green" => egui::Color32::from_rgb(60, 145, 80),
        "blue" => egui::Color32::from_rgb(70, 120, 190),
        "purple" => egui::Color32::from_rgb(130, 90, 190),
        "pink" => egui::Color32::from_rgb(180, 85, 150),
        "brown" => egui::Color32::from_rgb(130, 95, 70),
        "gray" | "default" => egui::Color32::from_gray(95),
        _ => colors::ACCENT,
    }
}

#[cfg(test)]
mod tests {
    use crate::notion::{AssetStatus, StatusGroup};
    use std::collections::HashSet;

    #[test]
    fn author_filter_matches_any_person_in_multi_author_combination() {
        assert!(super::author_matches_filter("Alice, Bob", "Alice"));
        assert!(super::author_matches_filter("Alice, Bob", "Bob"));
        assert!(!super::author_matches_filter("Alice, Bob", "Carol"));
    }

    #[test]
    fn status_filter_matches_selected_status_groups() {
        let status = Some(AssetStatus {
            id: "a".into(),
            name: "Creative review".into(),
            color: "blue".into(),
            group: StatusGroup::InProgress,
        });

        assert!(super::status_matches_filter(
            &status,
            &[StatusGroup::InProgress]
        ));
        assert!(!super::status_matches_filter(
            &status,
            &[StatusGroup::Complete]
        ));
        assert!(!super::status_matches_filter(
            &None,
            &[StatusGroup::InProgress]
        ));
    }

    #[test]
    fn published_slug_warning_only_shows_for_non_complete_statuses() {
        let published = crate::polyhaven::PublishedAssets {
            slugs: HashSet::from(["known_slug".to_string()]),
        };
        let in_progress = Some(AssetStatus {
            id: "a".into(),
            name: "Creative review".into(),
            color: "blue".into(),
            group: StatusGroup::InProgress,
        });
        let complete = Some(AssetStatus {
            id: "b".into(),
            name: "Done".into(),
            color: "green".into(),
            group: StatusGroup::Complete,
        });

        assert!(super::should_warn_published_slug(
            &published,
            "known_slug",
            &in_progress
        ));
        assert!(!super::should_warn_published_slug(
            &published,
            "known_slug",
            &complete
        ));
        assert!(!super::should_warn_published_slug(
            &published,
            "other_slug",
            &in_progress
        ));
    }
}
