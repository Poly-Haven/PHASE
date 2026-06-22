use super::colors;
use super::{layout, ActionPreview, AppState, AssetListState, RowKey, TransferAction, TransferKind};
use crate::copy::plan::Direction;
use crate::notion::{Asset, AssetStatus, AuthorProfile, StatusGroup, StatusOption};
use crate::ui::AssetType;

/// Severity of a row-level message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Warning,
    Error,
    #[allow(dead_code)]
    Question,
}

#[derive(Clone)]
pub enum RowMsgAction {
    CreateProdFolder,
    DeleteLocalFiles,
    /// Rename the Notion page title to this fixed slug.
    RenameTitle(String),
    /// Copy this asset's Prod files to the archive drive, then delete Prod.
    Archive,
    /// Open the Notion database for this asset type in a browser (shown on
    /// orphan rows, which have no card yet).
    OpenNotion,
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
    let search_query = state.search_query.clone();
    let mut rows = Vec::new();
    let mut status_options = Vec::new();
    let mut has_loaded_list = false;
    let mut loading_count = 0;
    let mut errors = Vec::new();

    for t in selected_types.iter().copied() {
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
                        .filter(|a| {
                            asset_matches_filters(a, &filters, &status_groups)
                                && super::slug_matches_search(&a.slug, &search_query)
                        })
                        .map(|a| {
                            let key = super::RowKey {
                                asset_type: t,
                                slug: a.slug.clone(),
                            };
                            let exists = *state.prod_folder_cache.get(&key).unwrap_or(&false);
                            let local_exists =
                                *state.local_folder_cache.get(&key).unwrap_or(&false);
                            let archive_exists =
                                *state.archive_folder_cache.get(&key).unwrap_or(&false);
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
                                archive_exists,
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

    // Orphan rows: folders on disk (Local/Prod/Archive) with no Notion card.
    for t in selected_types.iter().copied() {
        if let Some(orphans) = state.orphans_by_type.get(&t) {
            for orphan in orphans {
                if orphan_matches_filters(orphan, &filters, &status_groups, &search_query) {
                    rows.push(RowView::from_orphan(t, orphan));
                }
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
                let row_height = layout::ROW_HEIGHT;
                let top = ui.cursor().min;
                let row_rect = egui::Rect::from_min_size(top, egui::vec2(avail_w, row_height));
                if ui.is_rect_visible(row_rect) {
                    draw_row(state, ui, &key, row);
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

struct RowView {
    asset_type: super::AssetType,
    slug: String,
    author: String,
    author_profiles: Vec<AuthorProfile>,
    url: String,
    page_id: String,
    status: Option<AssetStatus>,
    exists_on_prod: bool,
    exists_on_archive: bool,
    exists_local: bool,
    messages: Vec<RowMsg>,
    /// True for folders discovered on disk with no Notion card. Notion-dependent
    /// UI (status pill, authors, Notion link) is hidden for these rows.
    is_orphan: bool,
}

impl RowView {
    #[allow(clippy::too_many_arguments)]
    fn from_asset(
        asset_type: super::AssetType,
        a: &Asset,
        exists_on_prod: bool,
        exists_local: bool,
        exists_on_archive: bool,
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
        let is_complete = a
            .status
            .as_ref()
            .map(|s| s.group == StatusGroup::Complete)
            .unwrap_or(false);
        if let Some(text) = crate::slug::message(&a.slug) {
            let fixed = crate::slug::fix(&a.slug);
            let (action, display_text) = if !fixed.is_empty() && fixed != a.slug {
                (Some(RowMsgAction::RenameTitle(fixed)), format!("{text} ·"))
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
        // "No prod folder" is suppressed once an asset has been archived (its
        // Prod files were intentionally moved to the archive drive).
        if !exists_on_prod && !exists_on_archive {
            messages.push(RowMsg {
                kind: MsgKind::Warning,
                text: "No prod folder.".into(),
                link: None,
                action: Some(RowMsgAction::CreateProdFolder),
                dismiss_key: None,
            });
        }
        // A finished asset that still has Prod files: offer to archive them
        // inline (same action as the context-menu item). Shown even when an
        // archive folder already exists — re-archiving skips unchanged files
        // and adds/updates the rest.
        if is_complete && exists_on_prod {
            messages.push(RowMsg {
                kind: MsgKind::Info,
                text: "Published.".into(),
                link: None,
                action: Some(RowMsgAction::Archive),
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
        if exists_local
            && crate::validation::status_has_passed_review(a.status.as_ref(), status_options)
        {
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
            author_profiles: a.author_profiles.clone(),
            url: a.url.clone(),
            page_id: a.page_id.clone(),
            status: a.status.clone(),
            exists_on_prod,
            exists_on_archive,
            exists_local,
            messages,
            is_orphan: false,
        }
    }

    /// Build a row for an orphan folder (no Notion card). Carries a synthetic
    /// location-derived status (so it sorts/filters by group) but no Notion
    /// identity; the renderer hides Notion-dependent UI for `is_orphan` rows.
    fn from_orphan(asset_type: super::AssetType, orphan: &super::OrphanAsset) -> Self {
        let mut messages = Vec::new();
        // Surface an obviously-broken folder name, but without the rename action
        // (there's no Notion title to rename).
        if let Some(text) = crate::slug::message(&orphan.slug) {
            messages.push(RowMsg {
                kind: MsgKind::Error,
                text,
                link: None,
                action: None,
                dismiss_key: None,
            });
        }
        messages.push(RowMsg {
            kind: MsgKind::Error,
            text: "No notion card.".into(),
            link: None,
            action: Some(RowMsgAction::OpenNotion),
            dismiss_key: None,
        });
        Self {
            asset_type,
            slug: orphan.slug.clone(),
            author: String::new(),
            author_profiles: Vec::new(),
            url: String::new(),
            page_id: String::new(),
            status: Some(AssetStatus {
                id: String::new(),
                name: String::new(),
                color: String::new(),
                group: orphan.group(),
                // Sorts orphans to the end of their asset type (after real
                // statuses, which have concrete sort orders).
                sort_order: usize::MAX,
            }),
            exists_on_prod: orphan.exists_prod,
            exists_on_archive: orphan.exists_archive,
            exists_local: orphan.exists_local,
            messages,
            is_orphan: true,
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
        if filtered
            .iter()
            .any(|msg| matches!(msg.kind, MsgKind::Error))
        {
            filtered
                .into_iter()
                .filter(|msg| matches!(msg.kind, MsgKind::Error))
                .collect()
        } else {
            filtered
        }
    }

    /// True when the asset's status is in the Complete group ("Done", etc.).
    fn is_complete(&self) -> bool {
        self.status
            .as_ref()
            .map(|s| s.group == StatusGroup::Complete)
            .unwrap_or(false)
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

const TRANSFER_ACTION_HOVER: egui::Color32 = egui::Color32::from_rgb(255, 214, 64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActionAvailability {
    enabled: bool,
    tooltip: &'static str,
}

fn row_action_availability(
    action: RowAction,
    has_prod_folder: bool,
    has_local_folder: bool,
    preview: Option<ActionPreview>,
) -> ActionAvailability {
    match action {
        RowAction::Push if !has_local_folder => ActionAvailability {
            enabled: false,
            tooltip: "Local folder missing",
        },
        RowAction::Push => match preview {
            Some(p) if p.file_count == 0 => ActionAvailability {
                enabled: false,
                tooltip: "Nothing new to push",
            },
            _ => ActionAvailability {
                enabled: true,
                tooltip: "Push to Prod",
            },
        },
        RowAction::Pull if !has_prod_folder => ActionAvailability {
            enabled: false,
            tooltip: "No prod folder",
        },
        RowAction::Pull => match preview {
            Some(p) if p.file_count == 0 => ActionAvailability {
                enabled: false,
                tooltip: "Nothing new to pull",
            },
            _ => ActionAvailability {
                enabled: true,
                tooltip: "Pull from Prod",
            },
        },
    }
}

/// Formats an action preview as "Push/Pull N file(s) · X.X GB".
fn fmt_action_preview(direction: Direction, preview: ActionPreview) -> String {
    let prefix = match direction {
        Direction::Push => "Push",
        Direction::Pull => "Pull",
    };
    let files = if preview.file_count == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", preview.file_count)
    };
    format!("{prefix} {files} · {}", fmt_bytes(preview.bytes))
}

fn icon_button(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    enabled: bool,
    tint_color: egui::Color32,
    tooltip: &str,
) -> egui::Response {
    let icon_size = egui::vec2(layout::ACTION_ICON_SIZE, layout::ACTION_ICON_SIZE);
    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(icon_size, sense);
    if ui.is_rect_visible(rect) {
        let tint = if !enabled {
            colors::TEXT_DISABLED
        } else if resp.hovered() {
            colors::HOVER
        } else {
            tint_color
        };
        ui.painter().image(tex.id(), rect, uv_full, tint);
    }
    let cursor = if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::NotAllowed
    };
    resp.on_hover_text(tooltip).on_hover_cursor(cursor)
}

fn transfer_action_button(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    enabled: bool,
    tint_color: egui::Color32,
    tooltip: &str,
    label: Option<&str>,
) -> egui::Response {
    let icon_size = egui::vec2(layout::ACTION_ICON_SIZE, layout::ACTION_ICON_SIZE);
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let label_size = label
        .map(|label| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(label.to_owned(), font_id.clone(), egui::Color32::WHITE)
                    .rect
                    .size()
            })
        })
        .unwrap_or(egui::Vec2::ZERO);
    let label_gap = if label.is_some() {
        layout::ROW_INTRA_ICON_GAP
    } else {
        0.0
    };
    let total_size = egui::vec2(
        icon_size.x + label_gap + label_size.x,
        layout::ROW_SECONDARY_HEIGHT,
    );
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(total_size, sense);
    if ui.is_rect_visible(rect) {
        let tint = if !enabled {
            colors::TEXT_DISABLED
        } else if response.hovered() {
            TRANSFER_ACTION_HOVER
        } else {
            tint_color
        };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - icon_size.x / 2.0, rect.center().y),
            icon_size,
        );
        ui.painter().image(
            tex.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
        if let Some(label) = label {
            ui.painter().text(
                egui::pos2(rect.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                font_id,
                tint,
            );
        }
    }
    let cursor = if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::NotAllowed
    };
    response.on_hover_text(tooltip).on_hover_cursor(cursor)
}

fn transfer_action_menu_label(action: TransferAction, preview: Option<ActionPreview>) -> String {
    let label = format!("{}{}", action.menu_label(), action.default_suffix());
    match preview {
        Some(preview) if preview.file_count > 0 => format!(
            "{label} · {} files · {}",
            preview.file_count,
            fmt_bytes(preview.bytes)
        ),
        _ => format!("{label} · {}", action.nothing_label()),
    }
}

fn transfer_action_menu_width(ui: &egui::Ui, labels: &[String]) -> f32 {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let max_label = labels
        .iter()
        .map(|label| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(label.clone(), font_id.clone(), egui::Color32::WHITE)
                    .rect
                    .width()
            })
        })
        .fold(0.0, f32::max);
    (max_label + layout::STATUS_OPTION_WIDTH_PADDING).max(layout::ROW_CONTEXT_POPUP_WIDTH)
}

fn draw_transfer_action_menu(
    ui: &mut egui::Ui,
    state: &mut AppState,
    key: &RowKey,
    action: TransferAction,
    preview: Option<ActionPreview>,
) {
    let label = transfer_action_menu_label(action, preview);
    let enabled = preview
        .map(|preview| preview.file_count > 0)
        .unwrap_or(false);
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        super::start_job(state, key, action);
        ui.close_menu();
    }
}

/// Resolve an asset's primary editable file under `folder` (the slug folder),
/// if it exists: `staging/{slug}.exr` (or `.hdr`) for HDRIs, `staging/{slug}.blend`
/// for Textures. Returns `None` when the file is not present.
pub(super) fn asset_file_path(
    asset_type: AssetType,
    folder: &std::path::Path,
    slug: &str,
) -> Option<std::path::PathBuf> {
    let staging = folder.join("staging");
    let candidate = match asset_type {
        AssetType::Hdris => {
            let exr = staging.join(format!("{slug}.exr"));
            if exr.is_file() {
                exr
            } else {
                staging.join(format!("{slug}.hdr"))
            }
        }
        AssetType::Textures => staging.join(format!("{slug}.blend")),
    };
    candidate.is_file().then_some(candidate)
}

fn open_asset_file(asset_type: AssetType, folder: &std::path::Path, slug: &str) {
    if let Some(file) = asset_file_path(asset_type, folder, slug) {
        let _ = open::that(file);
        return;
    }
    let staging = folder.join("staging");
    if staging.exists() {
        let _ = open::that(staging);
    } else {
        let _ = open::that(folder);
    }
}

fn thumbnail_source_path(prod_root: &std::path::Path, key: &RowKey) -> std::path::PathBuf {
    prod_root
        .join(key.asset_type.folder())
        .join(&key.slug)
        .join("staging")
        .join("renders")
        .join("primary.png")
}

struct RowLayout {
    thumbnail_rect: Option<egui::Rect>,
    content_min: egui::Pos2,
    content_w: f32,
}

fn row_layout(avail: egui::Rect, thumbnail_size: Option<egui::Vec2>) -> RowLayout {
    match thumbnail_size {
        Some(size) => {
            let thumbnail_rect = egui::Rect::from_min_size(avail.min, size);
            let content_min = egui::pos2(
                avail.min.x + size.x + layout::ROW_SECTION_PADDING,
                avail.min.y,
            );
            let content_w = (avail.width() - size.x - layout::ROW_SECTION_PADDING).max(0.0);
            RowLayout {
                thumbnail_rect: Some(thumbnail_rect),
                content_min,
                content_w,
            }
        }
        None => RowLayout {
            thumbnail_rect: None,
            content_min: avail.min,
            content_w: avail.width(),
        },
    }
}

fn transfer_progress_fill_rect(
    row_rect: egui::Rect,
    kind: TransferKind,
    fraction: f32,
) -> egui::Rect {
    let width = row_rect.width() * fraction.clamp(0.0, 1.0);
    // Push/Archive fill left-to-right (sending out); Pull/Unarchive fill
    // right-to-left (bringing in).
    match kind {
        TransferKind::Push | TransferKind::Archive => {
            egui::Rect::from_min_size(row_rect.min, egui::vec2(width, row_rect.height()))
        }
        TransferKind::Pull | TransferKind::Unarchive => egui::Rect::from_min_size(
            egui::pos2(row_rect.max.x - width, row_rect.min.y),
            egui::vec2(width, row_rect.height()),
        ),
    }
}

fn transfer_progress_color(kind: TransferKind) -> egui::Color32 {
    match kind {
        TransferKind::Push => colors::PUSH,
        TransferKind::Pull => colors::PULL,
        TransferKind::Archive | TransferKind::Unarchive => colors::STATUS_COMPLETE,
    }
}

fn draw_row(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let row_height = layout::ROW_HEIGHT;
    let avail = ui.available_rect_before_wrap();
    let avail_w = avail.width();
    let thumbnail_size = state
        .thumbnail_previews
        .get(key)
        .map(|preview| preview.texture.size_vec2());
    let row_layout = row_layout(avail, thumbnail_size);
    let row_rect = egui::Rect::from_min_size(avail.min, egui::vec2(avail_w, row_height));
    let row1_rect = egui::Rect::from_min_size(
        row_layout.content_min,
        egui::vec2(row_layout.content_w, layout::ROW_PRIMARY_HEIGHT),
    );
    let row2_rect = egui::Rect::from_min_size(
        row_layout.content_min + egui::vec2(0.0, layout::ROW_PRIMARY_HEIGHT),
        egui::vec2(row_layout.content_w, layout::ROW_SECONDARY_HEIGHT),
    );

    ui.painter()
        .rect_filled(row_rect, 2.0, colors::ROW_BACKGROUND);

    let prod_folder = state.prod_root_for(key.asset_type).join(&key.slug);
    let local_folder = state.local_root_for(key.asset_type).join(&key.slug);
    let archive_folder = state.archive_root_for(key.asset_type).join(&key.slug);
    let local_exists = row.exists_local;
    // A finished asset that lives only in the archive (its Prod files were moved
    // away). Such rows offer "Unarchive" instead of "Pull" and open the archive
    // folder instead of Prod.
    let is_archived_only = row.is_complete() && row.exists_on_archive && !row.exists_on_prod;

    let open_notion_in_app = state.config.open_notion_links_in_desktop_app;
    let row_response = ui.interact(
        row_rect,
        ui.id().with(("row-context", key.asset_type, &key.slug)),
        egui::Sense::hover(),
    );
    let is_complete = row.is_complete();
    row_response.context_menu(|ui| {
        super::scripts::draw_context_menu(
            ui,
            state,
            key,
            &row.url,
            open_notion_in_app,
            is_complete,
            row.exists_on_prod,
            row.is_orphan,
        );
    });

    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if let (Some(preview), Some(thumbnail_rect)) =
        (state.thumbnail_previews.get(key), row_layout.thumbnail_rect)
    {
        let thumbnail_response = ui.interact(
            thumbnail_rect,
            ui.id().with(("thumbnail", key.asset_type, &key.slug)),
            egui::Sense::click(),
        );
        ui.painter().image(
            preview.texture.id(),
            thumbnail_rect,
            uv_full,
            egui::Color32::WHITE,
        );
        if thumbnail_response.clicked() {
            let source_path = thumbnail_source_path(&state.config.prod_root, key);
            if source_path.is_file() {
                let _ = open::that(source_path);
            }
        }
        thumbnail_response
            .on_hover_text("Open source image on Prod")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    if let Some(job) = state.jobs.get(key) {
        let fill = transfer_progress_fill_rect(row_rect, job.kind, job.progress.fraction());
        ui.painter().rect_filled(
            fill,
            2.0,
            colors::colored_background(transfer_progress_color(job.kind)),
        );
    }

    // Row 1 LTR: status pill + bold slug
    ui.allocate_ui_at_rect(row1_rect, |ui| {
        ui.horizontal_centered(|ui| {
            ui.add_space(layout::ROW_SECTION_PADDING);
            // Orphans have no Notion status to show or change.
            if !row.is_orphan {
                draw_status_pill(state, ui, key, row);
                ui.add_space(layout::ROW_SECTION_PADDING);
            }
            let font_id = egui::TextStyle::Body.resolve(ui.style());
            let galley =
                ui.fonts(|f| f.layout_no_wrap(row.slug.clone(), font_id, egui::Color32::WHITE));
            let slug_size = galley.rect.size();
            let slug_start =
                egui::pos2(ui.cursor().min.x, row1_rect.center().y - slug_size.y / 2.0);
            let slug_rect = egui::Rect::from_min_size(slug_start, slug_size);
            let is_hovered = ui.rect_contains_pointer(slug_rect);
            let slug_color = if is_hovered {
                colors::HOVER
            } else {
                colors::TEXT_PRIMARY
            };
            let slug_resp = ui
                .add(
                    egui::Label::new(egui::RichText::new(&row.slug).strong().color(slug_color))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("Open asset file")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if slug_resp.clicked() {
                if row.exists_on_prod {
                    open_asset_file(row.asset_type, &prod_folder, &row.slug);
                } else {
                    handle_row_message_action(state, key, &RowMsgAction::CreateProdFolder);
                }
            }

            // Tiny grey copy button, superscript-style: snug against the slug's
            // top-right corner. Positioned manually (not as a flow item) so it
            // sits flush with the text instead of a gap away.
            let icon = layout::TINY_ICON_SIZE;
            let (copy_tex, copy_px) = super::copy_icon_texture(ui.ctx(), icon);
            // Blit 1:1 onto the physical pixel grid: snap the top-left to a whole
            // pixel and size the rect to exactly the rasterized texture, so the GPU
            // does no resampling (which is what caused the shimmer/aliasing).
            let ppp = ui.ctx().pixels_per_point();
            let logical = copy_px as f32 / ppp;
            let desired = egui::pos2(slug_resp.rect.right() + 4.0, slug_resp.rect.top() - 1.0);
            let snapped = egui::pos2(
                (desired.x * ppp).round() / ppp,
                (desired.y * ppp).round() / ppp,
            );
            let icon_rect = egui::Rect::from_min_size(snapped, egui::vec2(logical, logical));
            let copy_resp = ui
                .interact(
                    icon_rect,
                    ui.id().with(("copy-slug", key.asset_type, &key.slug)),
                    egui::Sense::click(),
                )
                .on_hover_text("Copy slug")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if ui.is_rect_visible(icon_rect) {
                let tint = if copy_resp.hovered() {
                    colors::HOVER
                } else {
                    colors::TEXT_DISABLED
                };
                ui.painter().image(
                    copy_tex.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    tint,
                );
            }
            if copy_resp.clicked() {
                ui.output_mut(|o| o.copied_text = row.slug.clone());
                state.row_toasts.insert(
                    key.clone(),
                    super::RowToast {
                        text: "Copied slug to clipboard".into(),
                        created_at: std::time::Instant::now(),
                    },
                );
            }

            // Orphans have no author data.
            if !row.is_orphan {
                ui.add_space(layout::ROW_SECTION_PADDING + 12.0);
                draw_row_authors(state, ui, row, colors::TEXT_PRIMARY);
            }
            super::scripts::draw_row_status(state, ui, key);
        });
    });

    // Row 1 RTL: push section (or planning / copy progress / archive-delete)
    if state.plan_jobs.contains_key(key) {
        ui.allocate_ui_at_rect(row1_rect, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(layout::ROW_SECTION_PADDING);
                ui.colored_label(colors::TEXT_DISABLED, "Planning…");
            });
        });
    } else if state.archive_deletes.contains_key(key) {
        ui.allocate_ui_at_rect(row1_rect, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(layout::ROW_SECTION_PADDING);
                ui.colored_label(colors::TEXT_DISABLED, "Removing Prod files…");
            });
        });
    } else if state.jobs.contains_key(key) {
        let label = state
            .jobs
            .get(key)
            .map(|j| j.kind.progressive())
            .unwrap_or("");
        let done = state
            .jobs
            .get(key)
            .map(|j| {
                j.progress
                    .bytes_done
                    .load(std::sync::atomic::Ordering::Relaxed)
            })
            .unwrap_or(0);
        let tot = state
            .jobs
            .get(key)
            .map(|j| {
                j.progress
                    .bytes_total
                    .load(std::sync::atomic::Ordering::Relaxed)
            })
            .unwrap_or(0);
        let x_tex = super::x_icon_texture(ui.ctx());
        ui.allocate_ui_at_rect(row1_rect, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(layout::ROW_SECTION_PADDING);
                if icon_button(ui, &x_tex, true, colors::TEXT_PRIMARY, "Cancel").clicked() {
                    if let Some(job) = state.jobs.get(key) {
                        job.progress
                            .cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                ui.label(format!("{label}  {} / {}", fmt_bytes(done), fmt_bytes(tot)));
            });
        });
    } else {
        let push_preview = state
            .transfer_estimates
            .get(&(key.clone(), TransferAction::PushAll))
            .copied();
        let push = row_action_availability(
            RowAction::Push,
            row.exists_on_prod,
            local_exists,
            push_preview,
        );
        let push_tex = super::push_icon_texture(ui.ctx());
        ui.allocate_ui_at_rect(row1_rect, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(layout::ROW_SECTION_PADDING);
                let push_label = push_preview
                    .filter(|preview| preview.file_count > 0)
                    .map(|preview| fmt_action_preview(Direction::Push, preview));
                let response = if let Some(label) = push_label.as_deref() {
                    transfer_action_button(
                        ui,
                        &push_tex,
                        push.enabled,
                        colors::PUSH,
                        push.tooltip,
                        Some(label),
                    )
                } else {
                    icon_button(ui, &push_tex, push.enabled, colors::PUSH, push.tooltip)
                };
                response.context_menu(|ui| {
                    let labels = [
                        transfer_action_menu_label(TransferAction::PushAll, push_preview),
                        transfer_action_menu_label(
                            TransferAction::PushStagingOnly,
                            state
                                .transfer_estimates
                                .get(&(key.clone(), TransferAction::PushStagingOnly))
                                .copied(),
                        ),
                    ];
                    ui.set_min_width(transfer_action_menu_width(ui, &labels));
                    draw_transfer_action_menu(
                        ui,
                        state,
                        key,
                        TransferAction::PushAll,
                        push_preview,
                    );
                    draw_transfer_action_menu(
                        ui,
                        state,
                        key,
                        TransferAction::PushStagingOnly,
                        state
                            .transfer_estimates
                            .get(&(key.clone(), TransferAction::PushStagingOnly))
                            .copied(),
                    );
                    ui.separator();
                    if ui.button("Show file list").clicked() {
                        state.open_transfer_file_list(key, Direction::Push);
                        ui.close_menu();
                    }
                });
                if response.clicked() {
                    super::start_job(state, key, TransferAction::PushAll);
                }
            });
        });
    }

    // Row 2 LTR: context btn + open-local btn + open-prod btn + notion btn + author + messages
    let folder_tex = super::folder_fill_texture(ui.ctx());
    let hdd_tex = super::hdd_network_texture(ui.ctx());
    let notion_tex = super::notion_logo_texture(ui.ctx());
    let text_color = if row.exists_on_prod {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_DISABLED
    };
    ui.allocate_ui_at_rect(row2_rect, |ui| {
        ui.horizontal_centered(|ui| {
            ui.add_space(layout::ROW_SECTION_PADDING);
            draw_row_context_button(state, ui, key, row);
            ui.add_space(layout::ROW_INTRA_ICON_GAP);
            if icon_button(
                ui,
                &folder_tex,
                local_exists,
                colors::TEXT_PRIMARY,
                "Open local folder",
            )
            .clicked()
            {
                let _ = open::that(&local_folder);
            }
            ui.add_space(layout::ROW_INTRA_ICON_GAP);
            if is_archived_only {
                let zip_tex = super::file_earmark_zip_texture(ui.ctx());
                if icon_button(
                    ui,
                    &zip_tex,
                    true,
                    colors::TEXT_PRIMARY,
                    "Open archive folder",
                )
                .clicked()
                {
                    let _ = open::that(&archive_folder);
                }
            } else if icon_button(
                ui,
                &hdd_tex,
                row.exists_on_prod,
                colors::TEXT_PRIMARY,
                "Open prod folder",
            )
            .clicked()
            {
                let _ = open::that(&prod_folder);
            }
            // The Notion link is meaningless for orphans (no card). The inline
            // "No notion card. Open Notion" message handles them instead.
            if !row.is_orphan {
                ui.add_space(layout::ROW_INTRA_ICON_GAP);
                let (notion_rect, notion_resp) = ui.allocate_exact_size(
                    egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                    egui::Sense::click(),
                );
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
                    open_notion_link(&row.url, open_notion_in_app);
                }
            }
            ui.add_space(layout::ROW_SECTION_PADDING);
            draw_row_messages(state, ui, key, row);
        });
    });

    // Row 2 RTL: unarchive button (for archived-only assets) or pull section.
    // Hidden while any plan/copy/archive-delete job is running for this row.
    let row_busy = state.plan_jobs.contains_key(key)
        || state.jobs.contains_key(key)
        || state.archive_deletes.contains_key(key);
    if !row_busy && is_archived_only {
        let unarchive_tex = super::unarchive_icon_texture(ui.ctx());
        ui.allocate_ui_at_rect(row2_rect, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(layout::ROW_SECTION_PADDING);
                if icon_button(
                    ui,
                    &unarchive_tex,
                    true,
                    colors::PULL,
                    "Unarchive (copy from archive back to Prod)",
                )
                .clicked()
                {
                    super::start_unarchive(state, key);
                }
            });
        });
    } else if !row_busy {
        let pull_preview = state
            .transfer_estimates
            .get(&(key.clone(), TransferAction::default_pull()))
            .copied();
        let pull = row_action_availability(
            RowAction::Pull,
            row.exists_on_prod,
            local_exists,
            pull_preview,
        );
        let pull_tex = super::pull_icon_texture(ui.ctx());
        ui.allocate_ui_at_rect(row2_rect, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(layout::ROW_SECTION_PADDING);
                let pull_label = pull_preview
                    .filter(|preview| preview.file_count > 0)
                    .map(|preview| fmt_action_preview(Direction::Pull, preview));
                let response = if let Some(label) = pull_label.as_deref() {
                    transfer_action_button(
                        ui,
                        &pull_tex,
                        pull.enabled,
                        colors::PULL,
                        pull.tooltip,
                        Some(label),
                    )
                } else {
                    icon_button(ui, &pull_tex, pull.enabled, colors::PULL, pull.tooltip)
                };
                response.context_menu(|ui| {
                    let labels = [
                        transfer_action_menu_label(TransferAction::default_pull(), pull_preview),
                        transfer_action_menu_label(
                            TransferAction::PullStagingOnly,
                            state
                                .transfer_estimates
                                .get(&(key.clone(), TransferAction::PullStagingOnly))
                                .copied(),
                        ),
                        transfer_action_menu_label(
                            TransferAction::PullAll,
                            state
                                .transfer_estimates
                                .get(&(key.clone(), TransferAction::PullAll))
                                .copied(),
                        ),
                    ];
                    ui.set_min_width(transfer_action_menu_width(ui, &labels));
                    draw_transfer_action_menu(
                        ui,
                        state,
                        key,
                        TransferAction::default_pull(),
                        pull_preview,
                    );
                    draw_transfer_action_menu(
                        ui,
                        state,
                        key,
                        TransferAction::PullStagingOnly,
                        state
                            .transfer_estimates
                            .get(&(key.clone(), TransferAction::PullStagingOnly))
                            .copied(),
                    );
                    draw_transfer_action_menu(
                        ui,
                        state,
                        key,
                        TransferAction::PullAll,
                        state
                            .transfer_estimates
                            .get(&(key.clone(), TransferAction::PullAll))
                            .copied(),
                    );
                    ui.separator();
                    if ui.button("Show file list").clicked() {
                        state.open_transfer_file_list(key, Direction::Pull);
                        ui.close_menu();
                    }
                });
                if response.clicked() {
                    super::start_job(state, key, TransferAction::default_pull());
                }
            });
        });
    }

    // A slim progress bar along the row's bottom edge during a *separate* verify
    // step (today only a Push's post-copy verification). The main copy progress
    // uses the full-row wash above; this small bar is reserved for verification.
    if let Some(verify) = state.verifications.get(key) {
        let color = transfer_progress_color(verify.kind);
        let h = layout::ROW_PROGRESS_BAR_HEIGHT;
        let track = egui::Rect::from_min_max(
            egui::pos2(row_rect.min.x, row_rect.max.y - h),
            row_rect.max,
        );
        ui.painter()
            .rect_filled(track, 0.0, colors::colored_background(color));
        let fill_w = row_rect.width() * verify.progress.fraction().clamp(0.0, 1.0);
        let fill = egui::Rect::from_min_size(
            egui::pos2(row_rect.min.x, row_rect.max.y - h),
            egui::vec2(fill_w, h),
        );
        ui.painter().rect_filled(fill, 0.0, color);
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
        ui.add_space(layout::ROW_SECTION_PADDING);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x =
                (ui.spacing().item_spacing.x - layout::ROW_INTRA_ICON_GAP).max(0.0);
            let (tex, color) = match msg.kind {
                MsgKind::Info => (super::info_icon_texture(ui.ctx()), colors::MSG_INFO),
                MsgKind::Warning => (super::warn_icon_texture(ui.ctx()), colors::MSG_WARNING),
                MsgKind::Error => (super::error_icon_texture(ui.ctx()), colors::MSG_ERROR),
                MsgKind::Question => (super::question_icon_texture(ui.ctx()), colors::MSG_QUESTION),
            };
            ui.add(
                egui::Image::new(egui::load::SizedTexture::new(
                    tex.id(),
                    egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                ))
                .tint(color),
            );
            ui.colored_label(color, &msg.text);
            if let Some(action) = &msg.action {
                let resp = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(action_label(action)).color(colors::HOVER),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                // Draw underline at 30% opacity instead of using .underline()
                let c = colors::HOVER;
                ui.painter().line_segment(
                    [
                        egui::pos2(resp.rect.min.x, resp.rect.max.y - 1.0),
                        egui::pos2(resp.rect.max.x, resp.rect.max.y - 1.0),
                    ],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(
                            c.r(),
                            c.g(),
                            c.b(),
                            (c.a() as f32 * 0.3) as u8,
                        ),
                    ),
                );
                if resp.clicked() {
                    handle_row_message_action(state, key, action);
                }
            }
            if let Some(link) = &msg.link {
                let tex = super::external_link_texture(ui.ctx());
                let resp = ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(layout::LINK_ICON_SIZE, layout::LINK_ICON_SIZE),
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
                        egui::vec2(layout::LINK_ICON_SIZE, layout::LINK_ICON_SIZE),
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

fn author_avatar_texture<'a>(
    state: &'a mut AppState,
    ctx: &egui::Context,
    author: &AuthorProfile,
) -> Option<&'a egui::TextureHandle> {
    let cache_key = crate::notion::author_avatar_cache_key(author);
    if !state.author_avatar_textures.contains_key(&cache_key) {
        let cache_path = crate::notion::author_avatar_cache_path(author)?;
        if !cache_path.is_file() {
            return None;
        }
        let texture_name = format!("author-avatar-{cache_key}");
        let texture = load_circular_avatar_texture(ctx, &cache_path, &texture_name).ok()?;
        state
            .author_avatar_textures
            .insert(cache_key.clone(), texture);
    }
    state.author_avatar_textures.get(&cache_key)
}

fn load_circular_avatar_texture(
    ctx: &egui::Context,
    cache_path: &std::path::Path,
    texture_name: &str,
) -> Result<egui::TextureHandle, String> {
    let bytes = std::fs::read(cache_path).map_err(|err| err.to_string())?;
    let decoded = image::load_from_memory(&bytes).map_err(|err| err.to_string())?;
    let mut rgba = decoded.to_rgba8();
    mask_circle(&mut rgba);
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    Ok(ctx.load_texture(texture_name, image, egui::TextureOptions::LINEAR))
}

fn mask_circle(image: &mut image::RgbaImage) {
    let width = image.width() as f32;
    let height = image.height() as f32;
    let radius = width.min(height) / 2.0;
    let center_x = width / 2.0;
    let center_y = height / 2.0;

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - center_x;
        let dy = y as f32 + 0.5 - center_y;
        if (dx * dx + dy * dy).sqrt() > radius {
            pixel[3] = 0;
        }
    }
}

fn author_initials(name: &str) -> String {
    let mut parts = name.split_whitespace().filter(|part| !part.is_empty());
    let first = parts.next().and_then(|part| part.chars().next());
    let second = parts.next().and_then(|part| part.chars().next());
    match (first, second) {
        (Some(first), Some(second)) => format!("{first}{second}"),
        (Some(first), None) => first.to_string(),
        _ => "?".into(),
    }
}

fn draw_author_avatar(
    ui: &mut egui::Ui,
    state: &mut AppState,
    author: &AuthorProfile,
    text_color: egui::Color32,
) {
    let size = egui::vec2(layout::ACTION_ICON_SIZE, layout::ACTION_ICON_SIZE);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    if let Some(texture) = author_avatar_texture(state, ui.ctx(), author) {
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.circle_filled(
            rect.center(),
            rect.width() / 2.0,
            text_color.linear_multiply(0.12),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            author_initials(&author.name),
            egui::TextStyle::Small.resolve(ui.style()),
            text_color,
        );
    }
}

fn draw_row_authors(
    state: &mut AppState,
    ui: &mut egui::Ui,
    row: &RowView,
    text_color: egui::Color32,
) {
    if row.author_profiles.is_empty() {
        ui.colored_label(colors::TEXT_PRIMARY.linear_multiply(0.25), &row.author);
        return;
    }

    ui.horizontal(|ui| {
        for (index, author) in row.author_profiles.iter().enumerate() {
            if index > 0 {
                ui.add_space(layout::ROW_SECTION_PADDING);
            }
            ui.horizontal(|ui| {
                draw_author_avatar(ui, state, author, text_color);
                ui.add_space(-4.0);
                ui.colored_label(colors::TEXT_PRIMARY.linear_multiply(0.25), &author.name);
            });
        }
    });
}

fn action_label(action: &RowMsgAction) -> String {
    match action {
        RowMsgAction::CreateProdFolder => "Create?".to_string(),
        RowMsgAction::DeleteLocalFiles => "Delete local files?".to_string(),
        RowMsgAction::RenameTitle(slug) => format!("Rename to {slug}"),
        RowMsgAction::Archive => "Archive files?".to_string(),
        RowMsgAction::OpenNotion => "Open Notion".to_string(),
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
        RowMsgAction::Archive => {
            state.request_archive(key);
        }
        RowMsgAction::RenameTitle(new_title) => {
            if let Some(page_id) = state.assets_by_type.get(&key.asset_type).and_then(|s| {
                if let AssetListState::Loaded(list) = s {
                    list.assets
                        .iter()
                        .find(|a| a.slug == key.slug)
                        .map(|a| a.page_id.clone())
                } else {
                    None
                }
            }) {
                super::start_title_rename(state, key, &page_id, new_title);
            }
        }
        RowMsgAction::OpenNotion => {
            let _ = open::that(super::notion_redirect_url(key.asset_type));
        }
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

            ui.add_space(layout::TOP_BAR_EDGE_PADDING);
        }
    });
}

fn draw_row_context_button(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let icon_size = egui::vec2(layout::ACTION_ICON_SIZE, layout::ACTION_ICON_SIZE);
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
    let is_complete = row.is_complete();
    let exists_on_prod = row.exists_on_prod;
    egui::popup::popup_below_widget(ui, popup_id, &response, |ui| {
        ui.set_min_width(layout::ROW_CONTEXT_POPUP_WIDTH);
        super::scripts::draw_context_menu(
            ui,
            state,
            key,
            &row.url,
            state.config.open_notion_links_in_desktop_app,
            is_complete,
            exists_on_prod,
            row.is_orphan,
        );
    });
}

fn row_context_texture(ctx: &egui::Context) -> egui::TextureHandle {
    super::list_texture(ctx)
}

fn row_context_icon_rect(rect: egui::Rect) -> egui::Rect {
    rect.shrink(layout::ROW_CONTEXT_ICON_INSET)
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
    ui.add_space(layout::ROW_SECTION_PADDING);
    let tex = super::check_texture(ui.ctx());
    ui.add(
        egui::Image::new(egui::load::SizedTexture::new(
            tex.id(),
            egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
        ))
        .tint(color),
    );
    ui.add_space(layout::ROW_INTRA_ICON_GAP);
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
    (max_label + layout::STATUS_OPTION_WIDTH_PADDING).max(layout::STATUS_OPTION_MIN_WIDTH)
}

fn colored_status_option(
    ui: &mut egui::Ui,
    label: &str,
    bg: egui::Color32,
    width: f32,
) -> egui::Response {
    let height = layout::STATUS_OPTION_HEIGHT;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let bg = colors::colored_background(bg);
    let fill = if response.hovered() {
        bg.linear_multiply(1.25)
    } else {
        bg
    };
    ui.painter().rect_filled(
        rect.shrink2(egui::vec2(
            layout::STATUS_OPTION_INSET,
            layout::STATUS_OPTION_INSET,
        )),
        layout::STATUS_OPTION_ROUNDING,
        fill,
    );
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
    let icon_size = egui::vec2(layout::STATUS_PILL_ICON_SIZE, layout::STATUS_PILL_ICON_SIZE);
    let padding = egui::vec2(layout::STATUS_PILL_PADDING_X, layout::STATUS_PILL_PADDING_Y);
    let height = layout::STATUS_PILL_HEIGHT;
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
    author_matches_filter(&super::asset_author_source(asset), filters)
        && status_matches_filter(&asset.status, selected)
}

/// Whether an orphan row should be shown. Status-group and search filters always
/// apply (via the location-derived group). Orphans have no author, so the author
/// filter is bypassed for *local* orphans (always shown) but hides Prod/Archive
/// orphans whenever an author filter is active.
pub(super) fn orphan_matches_filters(
    orphan: &super::OrphanAsset,
    filters: &[String],
    selected: &[StatusGroup],
    search: &str,
) -> bool {
    selected.contains(&orphan.group())
        && super::slug_matches_search(&orphan.slug, search)
        && (orphan.exists_local || filters.is_empty())
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
            author_profiles: Vec::new(),
            url: String::new(),
            page_id: String::new(),
            status,
            exists_on_prod: true,
            exists_on_archive: false,
            exists_local: true,
            messages: Vec::new(),
            is_orphan: false,
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

    fn orphan(slug: &str, local: bool, prod: bool, archive: bool) -> crate::ui::OrphanAsset {
        crate::ui::OrphanAsset {
            slug: slug.into(),
            exists_local: local,
            exists_prod: prod,
            exists_archive: archive,
        }
    }

    #[test]
    fn orphan_filter_local_always_shown_regardless_of_author_filter() {
        let local = orphan("a", true, false, false);
        // Shown with an active author filter, since its group (In progress) is selected.
        assert!(super::orphan_matches_filters(
            &local,
            &["Alice".to_string()],
            &[StatusGroup::InProgress],
            "",
        ));
        // Still gated by the status-group filter.
        assert!(!super::orphan_matches_filters(
            &local,
            &[],
            &[StatusGroup::Complete],
            "",
        ));
    }

    #[test]
    fn orphan_filter_prod_and_archive_hidden_when_author_filter_active() {
        let prod = orphan("b", false, true, false);
        assert!(super::orphan_matches_filters(
            &prod,
            &[],
            &[StatusGroup::ToDo],
            "",
        ));
        assert!(!super::orphan_matches_filters(
            &prod,
            &["Alice".to_string()],
            &[StatusGroup::ToDo],
            "",
        ));

        let archive = orphan("c", false, false, true);
        assert!(super::orphan_matches_filters(
            &archive,
            &[],
            &[StatusGroup::Complete],
            "",
        ));
        assert!(!super::orphan_matches_filters(
            &archive,
            &["Alice".to_string()],
            &[StatusGroup::Complete],
            "",
        ));
    }

    #[test]
    fn orphan_filter_respects_search() {
        let o = orphan("wooden_floor", true, false, false);
        assert!(super::orphan_matches_filters(
            &o,
            &[],
            &[StatusGroup::InProgress],
            "wood",
        ));
        assert!(!super::orphan_matches_filters(
            &o,
            &[],
            &[StatusGroup::InProgress],
            "metal",
        ));
    }

    #[test]
    fn from_orphan_has_no_notion_identity_and_open_notion_message() {
        let row = super::RowView::from_orphan(crate::ui::AssetType::Hdris, &orphan("abc", false, false, true));
        assert!(row.is_orphan);
        assert!(row.page_id.is_empty() && row.url.is_empty());
        // Archive-only → Done.
        assert_eq!(row.status.as_ref().unwrap().group, StatusGroup::Complete);
        assert!(row.exists_on_archive && !row.exists_on_prod);
        assert!(row.messages.iter().any(|m| m.text == "No notion card."
            && matches!(m.action, Some(super::RowMsgAction::OpenNotion))));
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
    fn row_layout_keeps_text_flush_left_without_thumbnail() {
        let avail = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 52.0));
        let layout = super::row_layout(avail, None);

        assert_eq!(layout.thumbnail_rect, None);
        assert_eq!(layout.content_min, avail.min);
        assert_eq!(layout.content_w, avail.width());
    }

    #[test]
    fn row_layout_offsets_content_by_thumbnail_width_only_when_present() {
        let avail = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 52.0));
        let thumbnail_size = egui::vec2(96.0, 52.0);
        let layout = super::row_layout(avail, Some(thumbnail_size));

        assert_eq!(
            layout.thumbnail_rect,
            Some(egui::Rect::from_min_size(avail.min, thumbnail_size))
        );
        assert_eq!(
            layout.content_min,
            egui::pos2(10.0 + 96.0 + layout::ROW_SECTION_PADDING, 20.0)
        );
        assert_eq!(layout.content_w, 400.0 - 96.0 - layout::ROW_SECTION_PADDING);
    }

    #[test]
    fn push_progress_fills_left_to_right() {
        let row_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 52.0));

        let fill = super::transfer_progress_fill_rect(row_rect, TransferKind::Push, 0.25);

        assert_eq!(fill.min, egui::pos2(10.0, 20.0));
        assert_eq!(fill.max, egui::pos2(110.0, 72.0));
    }

    #[test]
    fn pull_progress_fills_right_to_left() {
        let row_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 52.0));

        let fill = super::transfer_progress_fill_rect(row_rect, TransferKind::Pull, 0.25);

        assert_eq!(fill.min, egui::pos2(310.0, 20.0));
        assert_eq!(fill.max, egui::pos2(410.0, 72.0));
    }

    #[test]
    fn archive_progress_fills_left_to_right_like_push() {
        let row_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 52.0));
        let fill = super::transfer_progress_fill_rect(row_rect, TransferKind::Archive, 0.25);
        assert_eq!(fill.min, egui::pos2(10.0, 20.0));
        assert_eq!(fill.max, egui::pos2(110.0, 72.0));
    }

    #[test]
    fn unarchive_progress_fills_right_to_left_like_pull() {
        let row_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 52.0));
        let fill = super::transfer_progress_fill_rect(row_rect, TransferKind::Unarchive, 0.25);
        assert_eq!(fill.min, egui::pos2(310.0, 20.0));
        assert_eq!(fill.max, egui::pos2(410.0, 72.0));
    }

    #[test]
    fn transfer_progress_color_matches_kind() {
        assert_eq!(super::transfer_progress_color(TransferKind::Push), colors::PUSH);
        assert_eq!(super::transfer_progress_color(TransferKind::Pull), colors::PULL);
        assert_eq!(
            super::transfer_progress_color(TransferKind::Archive),
            colors::STATUS_COMPLETE
        );
        assert_eq!(
            super::transfer_progress_color(TransferKind::Unarchive),
            colors::STATUS_COMPLETE
        );
    }

    #[test]
    fn asset_file_path_prefers_exr_then_hdr_for_hdris() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path();
        let staging = folder.join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        assert_eq!(
            super::asset_file_path(crate::ui::AssetType::Hdris, folder, "sun"),
            None
        );
        std::fs::write(staging.join("sun.hdr"), b"h").unwrap();
        assert_eq!(
            super::asset_file_path(crate::ui::AssetType::Hdris, folder, "sun"),
            Some(staging.join("sun.hdr"))
        );
        std::fs::write(staging.join("sun.exr"), b"e").unwrap();
        assert_eq!(
            super::asset_file_path(crate::ui::AssetType::Hdris, folder, "sun"),
            Some(staging.join("sun.exr"))
        );
    }

    #[test]
    fn asset_file_path_resolves_blend_for_textures() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path();
        let staging = folder.join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        assert_eq!(
            super::asset_file_path(crate::ui::AssetType::Textures, folder, "wood"),
            None
        );
        std::fs::write(staging.join("wood.blend"), b"b").unwrap();
        assert_eq!(
            super::asset_file_path(crate::ui::AssetType::Textures, folder, "wood"),
            Some(staging.join("wood.blend"))
        );
    }

    #[test]
    fn thumbnail_source_path_targets_prod_root() {
        let key = crate::ui::RowKey {
            asset_type: crate::ui::AssetType::Hdris,
            slug: "asset".into(),
        };

        assert_eq!(
            super::thumbnail_source_path(&std::path::PathBuf::from(r"C:\prod"), &key),
            std::path::PathBuf::from(r"C:\prod\HDRIs\asset\staging\renders\primary.png")
        );
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

        assert!(!crate::validation::status_has_passed_review(
            Some(&status("review", "Creative review")),
            &statuses
        ));
        assert!(crate::validation::status_has_passed_review(
            Some(&status("approved", "Approved")),
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
            authors: Vec::new(),
            author_profiles: Vec::new(),
            url: String::new(),
            status: Some(status("approved", "Approved")),
        };
        let row = RowView::from_asset(
            crate::ui::AssetType::Hdris,
            &asset,
            true,
            true,
            false,
            &statuses,
            &crate::polyhaven::PublishedAssets::default(),
            &[],
            &HashSet::new(),
        );

        assert!(row.messages.iter().any(|msg| msg.text == "Passed review;"));
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
        let availability = row_action_availability(RowAction::Push, true, false, None);

        assert!(!availability.enabled);
        assert_eq!(availability.tooltip, "Local folder missing");
    }

    #[test]
    fn push_action_is_enabled_when_local_folder_exists() {
        let availability = row_action_availability(RowAction::Push, true, true, None);

        assert!(availability.enabled);
        assert_eq!(availability.tooltip, "Push to Prod");
    }

    #[test]
    fn pull_action_still_depends_on_prod_folder() {
        let availability = row_action_availability(RowAction::Pull, false, false, None);

        assert!(!availability.enabled);
        assert_eq!(availability.tooltip, "No prod folder");
    }

    #[test]
    fn push_is_disabled_when_preview_has_no_files() {
        let preview = Some(ActionPreview {
            file_count: 0,
            bytes: 0,
        });
        let availability = row_action_availability(RowAction::Push, true, true, preview);

        assert!(!availability.enabled);
        assert_eq!(availability.tooltip, "Nothing new to push");
    }

    #[test]
    fn pull_is_disabled_when_preview_has_no_files() {
        let preview = Some(ActionPreview {
            file_count: 0,
            bytes: 0,
        });
        let availability = row_action_availability(RowAction::Pull, true, true, preview);

        assert!(!availability.enabled);
        assert_eq!(availability.tooltip, "Nothing new to pull");
    }

    #[test]
    fn push_is_enabled_when_preview_has_files() {
        let preview = Some(ActionPreview {
            file_count: 3,
            bytes: 1024,
        });
        let availability = row_action_availability(RowAction::Push, true, true, preview);

        assert!(availability.enabled);
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
    fn all_errors_remain_visible_while_non_errors_are_hidden() {
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
                kind: MsgKind::Error,
                text: "Missing /staging/sunny_field.exr or .hdr in Prod".into(),
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

        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|msg| matches!(msg.kind, MsgKind::Error)));
        assert_eq!(visible[0].text, "Unexpected root entries: renders");
        assert_eq!(visible[1].text, "Missing /staging/sunny_field.exr or .hdr in Prod");
    }
}
