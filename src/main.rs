mod cache;
mod config;
mod copy;
mod notion;
mod polyhaven;
mod slug;
mod ui;

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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(default_window_size())
            .with_min_inner_size([600.0, 400.0])
            .with_icon(load_app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "PHASE",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(App {
                state: AppState::new(cfg),
            })
        }),
    )
}

struct App {
    state: AppState,
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
        self.state.pump();
        ui::draw(&mut self.state, ctx);
        if !self.state.jobs.is_empty()
            || !self.state.notion_rx.is_empty()
            || !self.state.pending_notion.is_empty()
            || self.state.published_rx.is_some()
            || !self.state.status_updates.is_empty()
            || !self.state.row_toasts.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
