mod config;
mod copy;
mod cache;
mod notion;
mod ui;

use ui::AppState;

fn init_logging() {
    use simplelog::{LevelFilter, WriteLogger, Config as SlConfig};
    use std::fs::OpenOptions;
    let Ok(path) = config::log_path() else { return; };
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
    egui::IconData { rgba: image.into_raw(), width, height }
}

fn main() -> eframe::Result<()> {
    init_logging();
    log::info!("PHASE {} starting", env!("CARGO_PKG_VERSION"));
    let cfg = config::load().unwrap_or_default();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([600.0, 400.0])
            .with_icon(load_app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "PHASE",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(App { state: AppState::new(cfg) })
        }),
    )
}

struct App { state: AppState }

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.pump();
        ui::draw(&mut self.state, ctx);
        if !self.state.jobs.is_empty() || !self.state.notion_rx.is_empty()
            || !self.state.pending_notion.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
