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

#[derive(Clone)]
pub enum RowMsgAction {
    CreateProdFolder,
    DeleteLocalFiles,
    /// Rename the Notion page title to this fixed slug.
    RenameTitle(String),
}

/// A single message attached to an asset row.
#[derive(Clone)]
pub struct RowMsg {
    pub kind: MsgKind,
    pub text: String,
    pub link: Option<String>,
    pub action: Option<RowMsgAction>,
    pub dismiss_key: Option<String>,
}

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    let filters = state.author_filters.clone();
    let status_groups = state.selected_status_groups.clone();
    let selected_types = state.selected_types.clone();
    let mut rows = Vec::new();
    let mut status_options = Vec::new();
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
                status_options.extend(list.statuses.iter().cloned());
                rows.extend(
                    list.assets
                        .iter()
                        .filter(|a| asset_matches_filters(a, &filters, &status_groups))
                        .map(|a| {
                            let key = super::RowKey {
                                asset_type: t,
                                slug: a.slug.clone(),
                            };
                            let exists = *state.prod_folder_cache.get(&key).unwrap_or(&false);
                            let local_exists =
                                *state.local_folder_cache.get(&key).unwrap_or(&false);
                            let validation_findings = state
                                .validation_results
                                .get(&key)
                                .cloned()
                                .unwrap_or_default();
                            RowView::from_asset(
                                t,
                                a,
                                exists,
                                local_exists,
                                &list.statuses,
                                &state.published_assets,
                                &validation_findings,
                                &state.dismissed_warning_keys,
                            )
                        }),
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

    sort_rows(&mut rows, &status_options);

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let avail_w = ui.available_width();
            for row in &rows {
                let key = RowKey {
                    asset_type: row.asset_type,
                    slug: row.slug.clone(),
                };
                let two_rows = needs_second_row(row, avail_w, ui);
                let row_height = if two_rows { 56.0 } else { 28.0 };
                let top = ui.cursor().min;
                let row_rect = egui::Rect::from_min_size(top, egui::vec2(avail_w, row_height));
                if ui.is_rect_visible(row_rect) {
                    draw_row(state, ui, &key, row, two_rows);
                } else {
                    ui.advance_cursor_after_rect(row_rect);
                }
            }
        });
}

fn sort_rows(rows: &mut [RowView], status_options: &[StatusOption]) {
    rows.sort_by(|a, b| {
        a.asset_type
            .order()
            .cmp(&b.asset_type.order())
            .then_with(|| {
                status_order(&a.status, status_options)
                    .cmp(&status_order(&b.status, status_options))
            })
            .then_with(|| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()))
    });
}

fn status_order(status: &Option<AssetStatus>, status_options: &[StatusOption]) -> usize {
    let Some(status) = status else {
        return usize::MAX;
    };
    status_options
        .iter()
        .find(|option| option.id == status.id)
        .map(|option| option.sort_order)
        .or(Some(status.sort_order))
        .unwrap_or(usize::MAX)
}

fn status_has_passed_review(status: &Option<AssetStatus>, status_options: &[StatusOption]) -> bool {
    let Some(status) = status else {
        return false;
    };
    let Some(status_order) = status_options
        .iter()
        .find(|option| option.id == status.id)
        .map(|option| option.sort_order)
    else {
        return false;
    };
    let Some(review_order) = status_options
        .iter()
        .filter(|option| option.name.to_lowercase().contains("review"))
        .map(|option| option.sort_order)
        .max()
    else {
        return false;
    };
    status_order > review_order
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
        exists_on_prod: bool,
        exists_local: bool,
        status_options: &[StatusOption],
        published_assets: &crate::polyhaven::PublishedAssets,
        validation_findings: &[crate::validation::Finding],
        dismissed_warning_keys: &std::collections::HashSet<String>,
    ) -> Self {
        let mut messages = Vec::new();
        let key = RowKey {
            asset_type,
            slug: a.slug.clone(),
        };
        if let Some(text) = crate::slug::message(&a.slug) {
            let fixed = crate::slug::fix(&a.slug);
            let (action, display_text) = if !fixed.is_empty() && fixed != a.slug {
                (
                    Some(RowMsgAction::RenameTitle(fixed)),
                    format!("{text} ·"),
                )
            } else {
                (None, text)
            };
            messages.push(RowMsg {
                kind: MsgKind::Error,
                text: display_text,
                link: None,
                action,
                dismiss_key: None,
            });
        }
        if !exists_on_prod {
            messages.push(RowMsg {
                kind: MsgKind::Warning,
                text: "No prod folder.".into(),
                link: None,
                action: Some(RowMsgAction::CreateProdFolder),
                dismiss_key: None,
            });
        }
        if should_warn_published_slug(published_assets, &a.slug, &a.status) {
            messages.push(RowMsg {
                kind: MsgKind::Warning,
                text: "Published asset with this slug found".into(),
                link: Some(format!("https://polyhaven.com/a/{}", a.slug)),
                action: None,
                dismiss_key: None,
            });
        }
        if exists_local && status_has_passed_review(&a.status, status_options) {
            messages.push(RowMsg {
                kind: MsgKind::Info,
                text: "Passed review;".into(),
                link: None,
                action: Some(RowMsgAction::DeleteLocalFiles),
                dismiss_key: None,
            });
        }
        for finding in validation_findings {
            messages.push(RowMsg {
                kind: Self::msg_kind_from_validation_severity(finding.severity),
                text: finding.text.clone(),
                link: None,
                action: None,
                dismiss_key: finding
                    .dismiss_id
                    .map(|dismiss_id| crate::validation::dismissal_key(&key, dismiss_id)),
            });
        }
        let messages = Self::visible_row_messages(&messages, dismissed_warning_keys);
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

    fn visible_row_messages(
        messages: &[RowMsg],
        dismissed_warning_keys: &std::collections::HashSet<String>,
    ) -> Vec<RowMsg> {
        let filtered = messages
            .iter()
            .filter(|msg| {
                msg.dismiss_key
                    .as_ref()
                    .map(|key| !dismissed_warning_keys.contains(key))
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered
            .iter()
            .find(|msg| matches!(msg.kind, MsgKind::Error))
            .cloned()
            .map(|msg| vec![msg])
            .unwrap_or(filtered)
    }

    fn msg_kind_from_validation_severity(severity: crate::validation::Severity) -> MsgKind {
        match severity {
            crate::validation::Severity::Info => MsgKind::Info,
            crate::validation::Severity::Warning => MsgKind::Warning,
            crate::validation::Severity::Error => MsgKind::Error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Push,
    Pull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActionAvailability {
    enabled: bool,
    tooltip: &'static str,
}

fn row_action_availability(
    action: RowAction,
    has_prod_folder: bool,
    has_local_folder: bool,
) -> ActionAvailability {
    match action {
        RowAction::Push if !has_local_folder => ActionAvailability {
            enabled: false,
            tooltip: "Local folder missing",
        },
        RowAction::Push => ActionAvailability {
            enabled: true,
            tooltip: "Push to Prod",
        },
        RowAction::Pull if !has_prod_folder => ActionAvailability {
            enabled: false,
            tooltip: "No prod folder",
        },
        RowAction::Pull => ActionAvailability {
            enabled: true,
            tooltip: "Pull from Prod",
        },
    }
}

fn transfer_tooltip(base: &str, bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => format!("{base} · {}", fmt_bytes(bytes)),
        None => base.to_string(),
    }
}

fn draw_row(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView, two_rows: bool) {
    let row_height = if two_rows { 56.0 } else { 28.0 };
    let avail = ui.available_rect_before_wrap();
    let row_rect = egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), row_height));
    // Primary row always occupies the first 28 px.
    let primary_rect = egui::Rect::from_min_size(row_rect.min, egui::vec2(row_rect.width(), 28.0));

    ui.painter()
        .rect_filled(row_rect, 2.0, colors::ROW_BACKGROUND);
    if let Some(job) = state.jobs.get(key) {
        let f = job.progress.fraction().clamp(0.0, 1.0);
        let mut fill = primary_rect;
        fill.set_width(avail.width() * f);
        ui.painter()
            .rect_filled(fill, 2.0, colors::colored_background(colors::PROGRESS_BAR));
    }

    let prod_folder = state.prod_root_for(key.asset_type).join(&key.slug);
    let local_folder = state.local_root_for(key.asset_type).join(&key.slug);

    // Register the row-wide context menu BEFORE drawing children. egui's context_menu
    // internally re-interacts with Sense::click, so it must be set up first so that
    // child widgets (allocated later) take priority for primary-click events.
    let row_response = ui.interact(
        row_rect,
        ui.id().with(("row-context", key.asset_type, &key.slug)),
        egui::Sense::hover(),
    );
    row_response.context_menu(|ui| {
        draw_context_menu(
            ui,
            &local_folder,
            &prod_folder,
            &row.url,
            state.config.open_notion_links_in_desktop_app,
        )
    });

    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.allocate_ui_at_rect(primary_rect, |ui| {
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
            let slug_start = egui::pos2(
                ui.cursor().min.x,
                primary_rect.center().y - slug_size.y / 2.0,
            );
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

            // Asset page button — sits immediately to the right of the slug.
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
                    .on_hover_text("Open on Notion")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                open_notion_link(&row.url, state.config.open_notion_links_in_desktop_app);
            }

            ui.add_space(16.0);
            ui.colored_label(text_color.linear_multiply(0.8), &row.author);
            ui.add_space(8.0);
            draw_status_pill(state, ui, key, row);

            // Messages stay in the primary row when everything fits.
            if !two_rows {
                draw_row_messages(state, ui, key, row);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw_row_actions(state, ui, key, row);
            });
        });
    });

    // Secondary row: messages that didn't fit in the primary row.
    if two_rows {
        let secondary_rect = egui::Rect::from_min_size(
            row_rect.min + egui::vec2(0.0, 28.0),
            egui::vec2(row_rect.width(), 28.0),
        );
        ui.allocate_ui_at_rect(secondary_rect, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(8.0);
                draw_row_messages(state, ui, key, row);
            });
        });
    }

    ui.advance_cursor_after_rect(row_rect);
}

/// Draws the toast (if any) and validation messages for a row.
/// Called from both the single-row primary layout and the secondary overflow row.
fn draw_row_messages(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    if let Some(toast) = state.row_toasts.get(key) {
        draw_toast(ui, toast);
    }
    for msg in &row.messages {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = (ui.spacing().item_spacing.x - 4.0).max(0.0);
            let (tex, color) = match msg.kind {
                MsgKind::Info => (super::info_icon_texture(ui.ctx()), colors::MSG_INFO),
                MsgKind::Warning => (super::warn_icon_texture(ui.ctx()), colors::MSG_WARNING),
                MsgKind::Error => (super::error_icon_texture(ui.ctx()), colors::MSG_ERROR),
                MsgKind::Question => (super::question_icon_texture(ui.ctx()), colors::MSG_QUESTION),
            };
            ui.add(
                egui::Image::new(egui::load::SizedTexture::new(
                    tex.id(),
                    egui::vec2(14.0, 14.0),
                ))
                .tint(color),
            );
            ui.colored_label(color, &msg.text);
            if let Some(action) = &msg.action {
                let resp = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(action_label(action))
                                .underline()
                                .color(colors::HOVER),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    handle_row_message_action(state, key, action);
                }
            }
            if let Some(link) = &msg.link {
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
            if let Some(dismiss_key) = &msg.dismiss_key {
                let tex = super::x_icon_texture(ui.ctx());
                let resp = ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(12.0, 12.0),
                    ))
                    .tint(egui::Color32::WHITE)
                    .sense(egui::Sense::click()),
                );
                if resp
                    .on_hover_text("Dismiss")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    state.dismiss_warning(dismiss_key.clone());
                }
            }
        });
    }
}

fn action_label(action: &RowMsgAction) -> String {
    match action {
        RowMsgAction::CreateProdFolder => "Create?".to_string(),
        RowMsgAction::DeleteLocalFiles => "Delete local files?".to_string(),
        RowMsgAction::RenameTitle(slug) => format!("Rename to {slug}"),
    }
}

fn handle_row_message_action(state: &mut AppState, key: &RowKey, action: &RowMsgAction) {
    match action {
        RowMsgAction::CreateProdFolder => {
            state.pending_prod_folder_create = Some(key.clone());
        }
        RowMsgAction::DeleteLocalFiles => {
            state.pending_local_folder_delete = Some(key.clone());
        }
        RowMsgAction::RenameTitle(new_title) => {
            if let Some(page_id) = state
                .assets_by_type
                .get(&key.asset_type)
                .and_then(|s| {
                    if let AssetListState::Loaded(list) = s {
                        list.assets.iter().find(|a| a.slug == key.slug).map(|a| a.page_id.clone())
                    } else {
                        None
                    }
                })
            {
                super::start_title_rename(state, key, &page_id, new_title);
            }
        }
    }
}

/// Estimated primary-row width if messages stay on the first row.
///
/// Text widths must use the same `TextStyle`s as the widgets below. Live layout traces showed
/// that hard-coding a 14 px font overestimated typical rows by ~50 px because egui 0.27's default
/// `Body` and `Button` styles are 12.5 px.
///
/// Cursor constants are derived by tracing the actual egui cursor positions in `draw_row`:
///
///   LHS cursor after status pill (before messages):
///     add_space(8) + slug_w + isp(8) + notion(14) + isp(8) + add_space(16) + author_w
///     + isp(8) + add_space(8) + status_w + isp(8) = 78 + slug_w + author_w + status_w
///
///   RHS: pull icon left edge is at right_edge − 84:
///     add_space(8) + context(18) + isp(8) + add_space(6) + push(18) + isp(8) + pull(18) = 84
///
///   Per message in outer layout (add_space advances cursor directly; no isp before it):
///     add_space(8) + inner_horizontal_min_rect(icon14 + isp4 + text_w) + isp(8)
///     = 34 + text_w  (plus 16 per optional link/dismiss icon inside the inner horizontal)
fn row_layout_width(row: &RowView, ui: &egui::Ui) -> f32 {
    let body_font = egui::TextStyle::Body.resolve(ui.style());
    let button_font = egui::TextStyle::Button.resolve(ui.style());
    let mw = |text: String, font: &egui::FontId| -> f32 {
        ui.fonts(|f| {
            f.layout_no_wrap(text, font.clone(), egui::Color32::WHITE)
                .rect
                .width()
        })
    };

    let slug_w = mw(row.slug.clone(), &body_font);
    let author_w = mw(row.author.clone(), &body_font);
    // Status pill: text_w + icon(10) + padding(8)*3 (from status_pill_button)
    let status_w = match &row.status {
        Some(s) => mw(s.name.clone(), &button_font) + 34.0,
        None => mw("No status".to_owned(), &body_font),
    };

    // LHS: add_space(8) + slug + isp(8) + notion(14) + isp(8) + add_space(16) + author
    // + isp(8) + add_space(8) + status + isp(8) = 78 fixed
    let lhs_w = 78.0 + slug_w + author_w + status_w;

    // RHS: add_space(8) + context(18) + isp(8) + add_space(6) + push(18) + isp(8) + pull(18)
    // = 84; pull's left edge is at right_edge − 84
    let rhs_w = 84.0;

    // Per message: add_space(8) + inner_horizontal(icon14 + isp4 + text) + isp(8) = 34 + text
    // The inner horizontal reports min_rect width = 14 + 4 + text = 18 + text.
    // Optional link/dismiss each add isp(4) + icon(12) = 16 inside the inner horizontal.
    let msg_w: f32 = row
        .messages
        .iter()
        .map(|msg| {
            let tw = mw(msg.text.clone(), &body_font);
            let action_w = msg
                .action
                .as_ref()
                .map(|action| mw(action_label(action).to_string(), &body_font) + 4.0)
                .unwrap_or(0.0);
            34.0 + tw
                + action_w
                + if msg.link.is_some() { 16.0 } else { 0.0 }
                + if msg.dismiss_key.is_some() { 16.0 } else { 0.0 }
        })
        .sum();

    lhs_w + msg_w + rhs_w
}

fn needs_second_row(row: &RowView, available_width: f32, ui: &egui::Ui) -> bool {
    if row.messages.is_empty() {
        return false;
    }
    row_layout_width(row, ui) > available_width
}

fn draw_row_actions(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let icon_size = egui::vec2(18.0, 18.0);
    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let local_exists = state.local_folder_cache.get(key).copied().unwrap_or(false);

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
            let base_tint = if enabled {
                colors::TEXT_PRIMARY
            } else {
                colors::TEXT_DISABLED
            };
            let tint = if resp.hovered() && enabled {
                colors::HOVER
            } else {
                base_tint
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

    if state.plan_jobs.contains_key(key) {
        ui.colored_label(colors::TEXT_DISABLED, "Planning…");
        return;
    }

    if state.jobs.contains_key(key) {
        let x_tex = super::x_icon_texture(ui.ctx());
        if icon_button(ui, &x_tex, true, "Cancel").clicked() {
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
    let push = row_action_availability(RowAction::Push, row.exists_on_prod, local_exists);
    let pull = row_action_availability(RowAction::Pull, row.exists_on_prod, local_exists);
    let push_tex = super::push_icon_texture(ui.ctx());
    let push_estimate = state
        .transfer_estimates
        .get(&(key.clone(), Direction::Push))
        .copied();
    let push_tooltip = transfer_tooltip(push.tooltip, push_estimate);
    let push_response = icon_button(ui, &push_tex, push.enabled, &push_tooltip);
    if push_response.hovered() && push.enabled {
        state.start_transfer_estimate(key, Direction::Push);
    }
    if push_response.clicked() {
        super::start_job(state, key, Direction::Push);
    }
    let pull_tex = super::pull_icon_texture(ui.ctx());
    let pull_estimate = state
        .transfer_estimates
        .get(&(key.clone(), Direction::Pull))
        .copied();
    let pull_tooltip = transfer_tooltip(pull.tooltip, pull_estimate);
    let pull_response = icon_button(ui, &pull_tex, pull.enabled, &pull_tooltip);
    if pull_response.hovered() && pull.enabled {
        state.start_transfer_estimate(key, Direction::Pull);
    }
    if pull_response.clicked() {
        super::start_job(state, key, Direction::Pull);
    }
}

fn draw_status_pill(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let Some(status) = &row.status else {
        ui.colored_label(colors::TEXT_DISABLED, "No status");
        return;
    };

    let is_updating = state.status_updates.contains_key(key);
    let popup_id = ui.make_persistent_id(("status_popup", &key.asset_type, &key.slug));
    let response = status_pill_button(ui, status, is_updating);

    if response.clicked() && !is_updating {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }

    egui::popup::popup_below_widget(ui, popup_id, &response, |ui| {
        let options = status_options_for(state, key.asset_type);
        let popup_width = status_dropdown_width(ui, &options);
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
    open_notion_in_app: bool,
) {
    if local_folder.exists() && ui.button("Open local folder").clicked() {
        let _ = open::that(local_folder);
        ui.close_menu();
    }
    if prod_folder.exists() && ui.button("Open prod folder").clicked() {
        let _ = open::that(prod_folder);
        ui.close_menu();
    }
    if ui.button("Open on Notion").clicked() {
        open_notion_link(notion_url, open_notion_in_app);
        ui.close_menu();
    }
}

fn draw_row_context_button(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let icon_size = egui::vec2(18.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(icon_size, egui::Sense::click());
    let tex = row_context_texture(ui.ctx());
    let tint = if response.hovered() {
        colors::HOVER
    } else if row.exists_on_prod {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_DISABLED
    };
    ui.painter().image(
        tex.id(),
        row_context_icon_rect(rect),
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
        draw_context_menu(
            ui,
            &local_folder,
            &prod_folder,
            &row.url,
            state.config.open_notion_links_in_desktop_app,
        );
    });
}

fn row_context_texture(ctx: &egui::Context) -> egui::TextureHandle {
    super::list_texture(ctx)
}

fn row_context_icon_rect(rect: egui::Rect) -> egui::Rect {
    rect.shrink(1.0)
}

fn open_notion_link(url: &str, open_in_app: bool) {
    if url.is_empty() {
        return;
    }
    let target = if open_in_app {
        notion_app_url(url)
    } else {
        url.to_string()
    };
    let _ = open::that(target);
}

fn notion_app_url(url: &str) -> String {
    if url.starts_with("notion://") {
        return url.to_string();
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return format!("notion://{rest}");
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return format!("notion://{rest}");
    }
    format!("notion://{url}")
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

pub(super) fn asset_matches_filters(
    asset: &Asset,
    filters: &[String],
    selected: &[StatusGroup],
) -> bool {
    author_matches_filter(&asset.author, filters) && status_matches_filter(&asset.status, selected)
}

fn author_matches_filter(author: &str, filters: &[String]) -> bool {
    super::authors::contains_any(author, filters)
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
    use super::*;
    use crate::notion::{AssetStatus, StatusGroup, StatusOption};
    use std::collections::HashSet;

    fn row(slug: &str, status: Option<AssetStatus>) -> RowView {
        RowView {
            asset_type: crate::ui::AssetType::Hdris,
            slug: slug.into(),
            author: String::new(),
            url: String::new(),
            page_id: String::new(),
            status,
            exists_on_prod: true,
            messages: Vec::new(),
        }
    }

    fn status(id: &str, name: &str) -> AssetStatus {
        AssetStatus {
            id: id.into(),
            name: name.into(),
            color: "default".into(),
            group: StatusGroup::InProgress,
            sort_order: 10,
        }
    }

    #[test]
    fn author_filter_matches_any_person_in_multi_author_combination() {
        assert!(super::author_matches_filter(
            "Alice, Bob",
            &["Alice".to_string()]
        ));
        assert!(super::author_matches_filter(
            "Alice, Bob",
            &["Bob".to_string()]
        ));
        assert!(!super::author_matches_filter(
            "Alice, Bob",
            &["Carol".to_string()]
        ));
    }

    #[test]
    fn status_filter_matches_selected_status_groups() {
        let status = Some(AssetStatus {
            id: "a".into(),
            name: "Creative review".into(),
            color: "blue".into(),
            group: StatusGroup::InProgress,
            sort_order: 20,
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
            sort_order: 10,
        });
        let complete = Some(AssetStatus {
            id: "b".into(),
            name: "Done".into(),
            color: "green".into(),
            group: StatusGroup::Complete,
            sort_order: 20,
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

    #[test]
    fn status_after_last_review_status_has_passed_review() {
        let statuses = vec![
            StatusOption {
                id: "todo".into(),
                name: "To-do".into(),
                color: "default".into(),
                group: StatusGroup::ToDo,
                sort_order: 30,
            },
            StatusOption {
                id: "review".into(),
                name: "Creative review".into(),
                color: "blue".into(),
                group: StatusGroup::InProgress,
                sort_order: 20,
            },
            StatusOption {
                id: "approved".into(),
                name: "Approved".into(),
                color: "green".into(),
                group: StatusGroup::InProgress,
                sort_order: 30,
            },
        ];

        assert!(!super::status_has_passed_review(
            &Some(status("review", "Creative review")),
            &statuses
        ));
        assert!(super::status_has_passed_review(
            &Some(status("approved", "Approved")),
            &statuses
        ));
    }

    #[test]
    fn passed_review_asset_with_local_folder_suggests_deleting_local_files() {
        let statuses = vec![
            StatusOption {
                id: "review".into(),
                name: "Creative review".into(),
                color: "blue".into(),
                group: StatusGroup::InProgress,
                sort_order: 20,
            },
            StatusOption {
                id: "approved".into(),
                name: "Approved".into(),
                color: "green".into(),
                group: StatusGroup::Complete,
                sort_order: 30,
            },
        ];
        let asset = Asset {
            page_id: "page".into(),
            slug: "asset".into(),
            author: "Author".into(),
            url: String::new(),
            status: Some(status("approved", "Approved")),
        };
        let row = RowView::from_asset(
            crate::ui::AssetType::Hdris,
            &asset,
            true,
            true,
            &statuses,
            &crate::polyhaven::PublishedAssets::default(),
            &[],
            &HashSet::new(),
        );

        assert!(row
            .messages
            .iter()
            .any(|msg| msg.text == "Passed review;"));
    }

    #[test]
    fn rows_sort_by_status_sort_order_then_slug_case_insensitive() {
        let status_options = vec![
            StatusOption {
                id: "creative-review".into(),
                name: "Creative review".into(),
                color: "blue".into(),
                group: StatusGroup::InProgress,
                sort_order: 20,
            },
            StatusOption {
                id: "awaiting-payment".into(),
                name: "Awaiting payment".into(),
                color: "yellow".into(),
                group: StatusGroup::InProgress,
                sort_order: 10,
            },
        ];
        let mut rows = vec![
            row(
                "zebra",
                Some(status("awaiting-payment", "Awaiting payment")),
            ),
            row("Beta", Some(status("creative-review", "Creative review"))),
            row("alpha", Some(status("creative-review", "Creative review"))),
            row("missing", None),
        ];

        sort_rows(&mut rows, &status_options);

        assert_eq!(
            rows.iter().map(|row| row.slug.as_str()).collect::<Vec<_>>(),
            vec!["zebra", "alpha", "Beta", "missing"]
        );
    }

    #[test]
    fn push_action_is_disabled_when_local_folder_is_missing() {
        let availability = row_action_availability(RowAction::Push, true, false);

        assert!(!availability.enabled);
        assert_eq!(availability.tooltip, "Local folder missing");
    }

    #[test]
    fn push_action_is_enabled_when_local_folder_exists() {
        let availability = row_action_availability(RowAction::Push, true, true);

        assert!(availability.enabled);
        assert_eq!(availability.tooltip, "Push to Prod");
    }

    #[test]
    fn pull_action_still_depends_on_prod_folder() {
        let availability = row_action_availability(RowAction::Pull, false, false);

        assert!(!availability.enabled);
        assert_eq!(availability.tooltip, "No prod folder");
    }

    #[test]
    fn transfer_tooltip_includes_size_when_known() {
        assert_eq!(
            transfer_tooltip("Pull from Prod", Some(1536)),
            "Pull from Prod · 2 KB"
        );
        assert_eq!(transfer_tooltip("Push to Prod", None), "Push to Prod");
    }

    #[test]
    fn dismissed_messages_are_filtered_out() {
        let messages = vec![
            RowMsg {
                kind: MsgKind::Warning,
                text: "Missing /staging/colorchart.zip in Prod".into(),
                link: None,
                action: None,
                dismiss_key: Some("HDRIs/sunny_field:missing-colorchart-zip".into()),
            },
            RowMsg {
                kind: MsgKind::Info,
                text: "Local files newer than Prod. Push?".into(),
                link: None,
                action: None,
                dismiss_key: None,
            },
        ];
        let dismissed = std::collections::HashSet::from([
            "HDRIs/sunny_field:missing-colorchart-zip".to_string(),
        ]);

        let visible = RowView::visible_row_messages(&messages, &dismissed);

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].text, "Local files newer than Prod. Push?");
    }

    #[test]
    fn first_error_hides_all_other_messages() {
        let messages = vec![
            RowMsg {
                kind: MsgKind::Warning,
                text: "Published asset with this slug found".into(),
                link: None,
                action: None,
                dismiss_key: None,
            },
            RowMsg {
                kind: MsgKind::Error,
                text: "Unexpected root entries: renders".into(),
                link: None,
                action: None,
                dismiss_key: None,
            },
            RowMsg {
                kind: MsgKind::Warning,
                text: "Missing /staging/colorchart.zip in Prod".into(),
                link: None,
                action: None,
                dismiss_key: Some("HDRIs/sunny_field:missing-colorchart-zip".into()),
            },
        ];

        let visible = RowView::visible_row_messages(&messages, &std::collections::HashSet::new());

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].text, "Unexpected root entries: renders");
        assert!(matches!(visible[0].kind, MsgKind::Error));
    }
}
