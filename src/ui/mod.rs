mod menu;
mod table;
mod dialogs;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use crate::config::Config;
use crate::copy::job::{JobMsg, JobProgress};
use crate::copy::plan::{build_plan, Action, Direction};
use crate::notion::{Asset, HDRIS_DB_ID, TEXTURES_DB_ID};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum AssetType { Hdris, Textures }

impl AssetType {
    pub fn label(self) -> &'static str { match self { Self::Hdris => "HDRIs", Self::Textures => "Textures" } }
    pub fn folder(self) -> &'static str { match self { Self::Hdris => "HDRIs", Self::Textures => "Textures" } }
    pub fn db_id(self) -> &'static str { match self { Self::Hdris => HDRIS_DB_ID, Self::Textures => TEXTURES_DB_ID } }
}

pub enum AssetListState {
    Loading,
    Loaded(Vec<Asset>),
    Error(String),
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct RowKey { pub asset_type: AssetType, pub slug: String }

pub struct RowJob {
    pub direction: Direction,
    pub progress:  Arc<JobProgress>,
    pub rx:        Receiver<JobMsg>,
    #[allow(dead_code)]
    pub message:   Arc<Mutex<String>>,
}

pub struct PendingConflict {
    pub key:       RowKey,
    pub direction: Direction,
    pub plan:      crate::copy::plan::Plan,
}

#[derive(Copy, Clone)]
pub enum ConflictChoice { OverwriteAll, CopyOnlyNew, Cancel }

pub struct AppState {
    pub config:         Config,
    pub current_type:   AssetType,
    pub author_filter:  String,
    pub assets_by_type: HashMap<AssetType, AssetListState>,
    pub error_banner:   Option<String>,
    pub jobs:           HashMap<RowKey, RowJob>,
    pub notion_rx:      HashMap<AssetType, Receiver<Result<Vec<Asset>, String>>>,
    pub pending_conflict: Option<PendingConflict>,
    pub token_prompt_open: bool,
    pub token_input:       String,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let mut s = Self {
            config,
            current_type: AssetType::Hdris,
            author_filter: String::new(),
            assets_by_type: HashMap::new(),
            error_banner: None,
            jobs: HashMap::new(),
            notion_rx: HashMap::new(),
            pending_conflict: None,
            token_prompt_open: false,
            token_input: String::new(),
        };
        s.token_prompt_open = s.config.notion_token.is_empty();
        s.token_input       = s.config.notion_token.clone();
        if !s.config.notion_token.is_empty() {
            s.refresh(AssetType::Hdris);
            s.refresh(AssetType::Textures);
        }
        s
    }

    pub fn refresh(&mut self, t: AssetType) {
        if self.config.notion_token.is_empty() {
            self.assets_by_type.insert(t, AssetListState::Error("No Notion token configured".into()));
            return;
        }
        self.assets_by_type.insert(t, AssetListState::Loading);
        let (tx, rx) = channel();
        let token = self.config.notion_token.clone();
        let db = t.db_id().to_string();
        thread::spawn(move || {
            let res = crate::notion::fetch_database(&token, &db).map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.notion_rx.insert(t, rx);
    }

    /// Drain Notion + job channels each frame.
    pub fn pump(&mut self) {
        let types: Vec<_> = self.notion_rx.keys().copied().collect();
        for t in types {
            let res_opt = self.notion_rx.get(&t).and_then(|rx| rx.try_recv().ok());
            if let Some(res) = res_opt {
                match res {
                    Ok(list) => { self.assets_by_type.insert(t, AssetListState::Loaded(list)); }
                    Err(msg) => { self.assets_by_type.insert(t, AssetListState::Error(msg)); }
                }
                self.notion_rx.remove(&t);
            }
        }

        let keys: Vec<RowKey> = self.jobs.keys().cloned().collect();
        for k in keys {
            let mut done = false;
            let mut err_msg: Option<String> = None;
            if let Some(job) = self.jobs.get(&k) {
                while let Ok(msg) = job.rx.try_recv() {
                    match msg {
                        JobMsg::FileDone { .. } => {}
                        JobMsg::FileFailed { rel_path, error } => {
                            err_msg = Some(format!("{}: {rel_path} — {error}", k.slug));
                            done = true;
                        }
                        JobMsg::Finished | JobMsg::Cancelled => { done = true; }
                    }
                }
            }
            if let Some(m) = err_msg { self.error_banner = Some(m); }
            if done { self.jobs.remove(&k); }
        }
    }

    pub fn local_root_for(&self, t: AssetType) -> PathBuf { self.config.local_root.join(t.folder()) }
    pub fn prod_root_for(&self,  t: AssetType) -> PathBuf { self.config.prod_root.join(t.folder()) }
}

pub fn start_job(state: &mut AppState, key: &RowKey, direction: Direction) {
    let (src_root, dst_root) = match direction {
        Direction::Pull => (state.prod_root_for(key.asset_type).join(&key.slug),
                            state.local_root_for(key.asset_type).join(&key.slug)),
        Direction::Push => (state.local_root_for(key.asset_type).join(&key.slug),
                            state.prod_root_for(key.asset_type).join(&key.slug)),
    };

    let plan = match build_plan(direction, &src_root, &dst_root) {
        Ok(p) => p,
        Err(e) => { state.error_banner = Some(format!("Plan failed: {e}")); return; }
    };

    if !plan.conflicts().is_empty() {
        state.pending_conflict = Some(PendingConflict { key: key.clone(), direction, plan });
        return;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan, progress.clone(), tx);
    state.jobs.insert(key.clone(), RowJob {
        direction,
        progress,
        rx,
        message: Arc::new(Mutex::new(String::new())),
    });
}

pub fn execute_after_conflict(state: &mut AppState, choice: ConflictChoice) {
    let Some(pc) = state.pending_conflict.take() else { return; };
    let mut plan = pc.plan;
    match choice {
        ConflictChoice::Cancel => return,
        ConflictChoice::OverwriteAll => {
            for f in plan.files.iter_mut() {
                if matches!(f.action, Action::Conflict { .. }) {
                    f.action = Action::Overwrite;
                    plan.total_bytes_to_copy += f.size;
                }
            }
        }
        ConflictChoice::CopyOnlyNew => {
            plan.files.retain(|f| !matches!(f.action, Action::Conflict { .. }));
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan, progress.clone(), tx);
    state.jobs.insert(pc.key, RowJob {
        direction: pc.direction,
        progress,
        rx,
        message: Arc::new(Mutex::new(String::new())),
    });
}

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    egui::TopBottomPanel::top("menu").show(ctx, |ui| menu::draw(state, ui));
    if let Some(err) = state.error_banner.clone() {
        egui::TopBottomPanel::top("banner").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                if ui.button("✕").clicked() { state.error_banner = None; }
            });
        });
    }
    dialogs::token_prompt(state, ctx);
    dialogs::draw(state, ctx);
    egui::CentralPanel::default().show(ctx, |ui| table::draw(state, ui));
}

pub fn notion_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/notion.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| {
        load_svg_texture(ctx, BYTES, "notion_logo", "notion.svg")
    }).clone()
}

pub fn pull_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/box-arrow-in-down.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "pull_icon", "box-arrow-in-down.svg")).clone()
}

pub fn push_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/cloud-upload-fill.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "push_icon", "cloud-upload-fill.svg")).clone()
}

fn load_svg_texture(
    ctx: &egui::Context,
    bytes: &[u8],
    texture_name: &'static str,
    debug_name: &'static str,
) -> egui::TextureHandle {
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    // Resolve `fill="currentColor"` to white so egui's `tint()` can multiply
    // it down to whatever colour the row needs at draw time.
    opt.style_sheet = Some("svg { color: #ffffff; }".to_string());

    let tree = usvg::Tree::from_data(bytes, &opt).expect(debug_name);
    let size = tree.size().to_int_size();
    // Render at 4x for crisp shrunken icons (the button is ~18px but the
    // source SVG may be 16px or 24px; oversampling avoids aliasing).
    let scale = 4u32;
    let w = size.width() * scale;
    let h = size.height() * scale;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect(debug_name);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );
    ctx.load_texture(
        texture_name,
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], pixmap.data()),
        egui::TextureOptions::LINEAR,
    )
}
