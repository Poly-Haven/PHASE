use super::{AppState, AssetListState, RowKey};
use super::colors;
use crate::copy::plan::Direction;
use crate::notion::Asset;

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    let t = state.current_type;
    match state.assets_by_type.get(&t) {
        None | Some(AssetListState::Loading) => { ui.label("Loading…"); return; }
        Some(AssetListState::Error(msg)) => { ui.colored_label(colors::ERROR_BANNER, msg.clone()); return; }
        Some(AssetListState::Loaded(_))      => {}
    }

    let prod_root = state.prod_root_for(t);
    let filter = state.author_filter.clone();
    let mut rows: Vec<RowView> = match state.assets_by_type.get(&t) {
        Some(AssetListState::Loaded(list)) => list.iter()
            .filter(|a| filter.is_empty() || a.author == filter)
            .map(|a| RowView::from_asset(a, &prod_root))
            .collect(),
        _ => Vec::new(),
    };
    rows.sort_by(|a, b| b.exists_on_prod.cmp(&a.exists_on_prod)
        .then_with(|| a.slug.to_lowercase().cmp(&b.slug.to_lowercase())));

    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        for row in rows {
            let key = RowKey { asset_type: t, slug: row.slug.clone() };
            draw_row(state, ui, &key, &row);
        }
    });
}

struct RowView {
    slug: String,
    author: String,
    url: String,
    exists_on_prod: bool,
}

impl RowView {
    fn from_asset(a: &Asset, prod_root: &std::path::Path) -> Self {
        Self {
            slug: a.slug.clone(),
            author: a.author.clone(),
            url: a.url.clone(),
            exists_on_prod: prod_root.join(&a.slug).is_dir(),
        }
    }
}

fn draw_row(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let row_height = 28.0;
    let avail = ui.available_rect_before_wrap();
    let row_rect = egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), row_height));

    let bg = ui.visuals().extreme_bg_color;
    ui.painter().rect_filled(row_rect, 2.0, bg);
    if let Some(job) = state.jobs.get(key) {
        let f = job.progress.fraction().clamp(0.0, 1.0);
        let mut fill = row_rect;
        fill.set_width(avail.width() * f);
        ui.painter().rect_filled(fill, 2.0, colors::PROGRESS_BAR);
    }

    let prod_folder = state.prod_root_for(key.asset_type).join(&key.slug);

    ui.allocate_ui_at_rect(row_rect, |ui| {
        ui.horizontal_centered(|ui| {
            ui.add_space(8.0);
            let text_color = if row.exists_on_prod { colors::SLUG_ACTIVE } else { colors::SLUG_MISSING };

            // Slug — clickable if prod folder exists, opens it in Explorer.
            let slug_label = egui::Label::new(
                egui::RichText::new(&row.slug).color(text_color)
            ).sense(egui::Sense::click());
            let slug_resp = ui.add(slug_label);
            if row.exists_on_prod {
                slug_resp
                    .on_hover_text("Open in Explorer")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                    .then(|| { let _ = open::that(&prod_folder); });
            }

            ui.add_space(16.0);
            ui.colored_label(text_color.linear_multiply(0.8), &row.author);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw_row_actions(state, ui, key, row);
            });
        });
    });

    ui.advance_cursor_after_rect(row_rect);
    ui.separator();
}

fn draw_row_actions(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let text_color = if row.exists_on_prod { colors::SLUG_ACTIVE } else { colors::SLUG_MISSING };
    let icon_size = egui::vec2(18.0, 18.0);

    let icon_button = |ui: &mut egui::Ui, tex: &egui::TextureHandle, enabled: bool, tooltip: &str| -> egui::Response {
        let btn = egui::ImageButton::new(egui::load::SizedTexture::new(tex.id(), icon_size))
            .frame(false)
            .tint(text_color);
        let cursor = if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed };
        ui.add_enabled(enabled, btn)
            .on_hover_text(tooltip)
            .on_hover_cursor(cursor)
    };

    if state.jobs.contains_key(key) {
        if ui.button("✕")
            .on_hover_text("Cancel")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            if let Some(job) = state.jobs.get(key) {
                job.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let job = state.jobs.get(key).unwrap();
        let done = job.progress.bytes_done.load(std::sync::atomic::Ordering::Relaxed);
        let tot  = job.progress.bytes_total.load(std::sync::atomic::Ordering::Relaxed);
        let label = match job.direction {
            Direction::Pull => "Pulling from Prod",
            Direction::Push => "Pushing from Local",
        };
        ui.label(format!("{label}  ·  {} / {}", fmt_bytes(done), fmt_bytes(tot)));
        return;
    }

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

fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = b as f64;
    if b >= GB { format!("{:.2} GB", b / GB) }
    else if b >= MB { format!("{:.1} MB", b / MB) }
    else if b >= KB { format!("{:.0} KB", b / KB) }
    else { format!("{b:.0} B") }
}
