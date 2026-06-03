#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod auth;
mod config;
mod copy;
mod notion;
mod polyhaven;
mod slug;
mod ui;
mod updater;
mod validation;

use ui::AppState;

fn init_logging() {
    use simplelog::{Config as SlConfig, LevelFilter, WriteLogger};
    use std::fs::OpenOptions;
    let Ok(path) = config::log_path() else {
        return;
    };
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = WriteLogger::init(LevelFilter::Info, SlConfig::default(), file);
    }
}

fn load_app_icon() -> egui::IconData {
    let bytes = include_bytes!("assets/app_logo.png");
    let image = image::load_from_memory(bytes)
        .expect("decode app_logo.png")
        .to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn default_window_size() -> [f32; 2] {
    [860.0, 500.0]
}

fn main() -> eframe::Result<()> {
    init_logging();
    log::info!("PHASE {} starting", env!("CARGO_PKG_VERSION"));
    let cfg = config::load().unwrap_or_default();

    let size = cfg.window_size.unwrap_or_else(default_window_size);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(size)
        .with_min_inner_size([600.0, 400.0])
        .with_icon(load_app_icon());
    if let Some([x, y]) = cfg.window_pos {
        viewport = viewport.with_position([x, y]);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "PHASE",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(App {
                state: AppState::new(cfg),
                last_window_pos: None,
                last_window_size: None,
            })
        }),
    )
}

struct App {
    state: AppState,
    last_window_pos: Option<egui::Pos2>,
    last_window_size: Option<egui::Vec2>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_window_size_is_compact() {
        assert_eq!(super::default_window_size(), [860.0, 500.0]);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.input(|i| {
            let vp = i.viewport();
            self.last_window_pos = vp.outer_rect.map(|r| r.min);
            if let Some(inner) = vp.inner_rect {
                self.last_window_size = Some(inner.size());
            }
        });

        self.state.pump();
        ui::draw(&mut self.state, ctx);
        if !self.state.jobs.is_empty()
            || !self.state.notion_rx.is_empty()
            || !self.state.pending_notion.is_empty()
            || self.state.published_rx.is_some()
            || !self.state.status_updates.is_empty()
            || !self.state.title_renames.is_empty()
            || self.state.auth_rx.is_some()
            || !self.state.row_toasts.is_empty()
            || self.state.validation_job.is_some()
            || !self.state.transfer_estimate_jobs.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let mut cfg = self.state.config.clone();
        if let Some(size) = self.last_window_size {
            cfg.window_size = Some([size.x, size.y]);
        }
        cfg.window_pos = self.last_window_pos.map(|p| [p.x, p.y]);
        let _ = config::save(&cfg);
    }
}
