mod asset_types;
mod authors;
pub mod colors;
mod dialogs;
mod focus_refresh;
mod group_selector;
mod loading_indicator;
mod menu;
mod status_groups;
mod table;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::copy::job::{JobMsg, JobProgress, VerifyMsg};
use crate::copy::plan::{build_plan, Action, Direction};
use crate::notion::{AssetList, AssetStatus, StatusOption, HDRIS_DB_ID, TEXTURES_DB_ID};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum AssetType {
    Hdris,
    Textures,
}

impl AssetType {
    pub fn all() -> &'static [Self] {
        &[Self::Hdris, Self::Textures]
    }
    pub fn order(self) -> usize {
        match self {
            Self::Hdris => 0,
            Self::Textures => 1,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Hdris => "HDRIs",
            Self::Textures => "Textures",
        }
    }
    pub fn folder(self) -> &'static str {
        match self {
            Self::Hdris => "HDRIs",
            Self::Textures => "Textures",
        }
    }
    pub fn db_id(self) -> &'static str {
        match self {
            Self::Hdris => HDRIS_DB_ID,
            Self::Textures => TEXTURES_DB_ID,
        }
    }
    pub fn cache_name(self) -> &'static str {
        match self {
            Self::Hdris => "hdris",
            Self::Textures => "textures",
        }
    }
    pub fn selected_color(self) -> egui::Color32 {
        match self {
            Self::Hdris => colors::ASSET_TYPE_HDRIS,
            Self::Textures => colors::ASSET_TYPE_TEXTURES,
        }
    }
    pub fn from_label(label: &str) -> Option<Self> {
        Self::all().iter().copied().find(|t| t.label() == label)
    }
}

pub enum AssetListState {
    Loading,
    Loaded(AssetList),
    Error(String),
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct RowKey {
    pub asset_type: AssetType,
    pub slug: String,
}

pub struct RowJob {
    pub direction: Direction,
    pub plan: crate::copy::plan::Plan,
    pub progress: Arc<JobProgress>,
    pub rx: Receiver<JobMsg>,
    pub started_at: Instant,
    #[allow(dead_code)]
    pub message: Arc<Mutex<String>>,
}

pub struct VerificationJob {
    pub progress: Arc<JobProgress>,
    pub rx: Receiver<VerifyMsg>,
}

pub struct PendingVerificationFailure {
    pub key: RowKey,
    pub rel_path: String,
    pub error: String,
}

pub struct RowToast {
    pub text: String,
    pub created_at: Instant,
}

pub struct PendingConflict {
    pub key: RowKey,
    pub direction: Direction,
    pub plan: crate::copy::plan::Plan,
}

pub struct StatusUpdateJob {
    pub rx: Receiver<Result<(), String>>,
    pub previous: Option<AssetStatus>,
    #[allow(dead_code)]
    pub requested: StatusOption,
}

#[derive(Copy, Clone)]
pub enum ConflictChoice {
    OverwriteAll,
    CopyOnlyNew,
    Cancel,
}

pub struct AppState {
    pub config: Config,
    pub current_type: AssetType,
    pub selected_types: Vec<AssetType>,
    pub selected_status_groups: Vec<crate::notion::StatusGroup>,
    pub author_filter: String,
    pub assets_by_type: HashMap<AssetType, AssetListState>,
    pub error_banner: Option<String>,
    pub jobs: HashMap<RowKey, RowJob>,
    pub verifications: HashMap<RowKey, VerificationJob>,
    pub status_updates: HashMap<RowKey, StatusUpdateJob>,
    pub notion_rx: HashMap<AssetType, Receiver<Result<AssetList, String>>>,
    pub pending_conflict: Option<PendingConflict>,
    pub pending_verification_failure: Option<PendingVerificationFailure>,
    pub pending_prod_folder_create: Option<RowKey>,
    pub row_toasts: HashMap<RowKey, RowToast>,
    pub published_assets: crate::polyhaven::PublishedAssets,
    pub published_rx: Option<Receiver<Result<crate::polyhaven::PublishedAssets, String>>>,
    pub refreshing_published: bool,
    pub token_prompt_open: bool,
    pub token_input: String,
    /// Asset types whose background fetch is currently in flight.
    pub refreshing: HashSet<AssetType>,
    /// Notion results buffered while the cursor is active in the table.
    pub pending_notion: HashMap<AssetType, AssetList>,
    /// Last time the pointer moved while inside the table area.
    pub cursor_moved_in_table_at: Option<Instant>,
    pub focus_refresh: focus_refresh::State,
    /// Cached result of `is_dir()` for each asset's prod folder.
    /// Rebuilt when Notion data loads, window gains focus, a job finishes,
    /// or a prod folder is created — never on every frame.
    pub prod_folder_cache: HashMap<RowKey, bool>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let selected_types = if config.last_asset_types.is_empty() {
            asset_types::from_labels(&[config.last_tab.clone()])
        } else {
            asset_types::from_labels(&config.last_asset_types)
        };
        let current_type = selected_types.first().copied().unwrap_or(AssetType::Hdris);
        let author_filter = if config.last_author_filter.is_empty() {
            config
                .last_filters
                .get(current_type.label())
                .cloned()
                .unwrap_or_default()
        } else {
            config.last_author_filter.clone()
        };

        let mut s = Self {
            current_type,
            selected_types,
            selected_status_groups: crate::notion::StatusGroup::default_filter(),
            author_filter,
            config,
            assets_by_type: HashMap::new(),
            error_banner: None,
            jobs: HashMap::new(),
            verifications: HashMap::new(),
            status_updates: HashMap::new(),
            notion_rx: HashMap::new(),
            pending_conflict: None,
            pending_verification_failure: None,
            pending_prod_folder_create: None,
            row_toasts: HashMap::new(),
            published_assets: crate::cache::load(crate::polyhaven::cache_name())
                .unwrap_or_default(),
            published_rx: None,
            refreshing_published: false,
            token_prompt_open: false,
            token_input: String::new(),
            refreshing: HashSet::new(),
            pending_notion: HashMap::new(),
            cursor_moved_in_table_at: None,
            focus_refresh: focus_refresh::State::default(),
            prod_folder_cache: HashMap::new(),
        };
        s.token_prompt_open = s.config.notion_token.is_empty();
        s.token_input = s.config.notion_token.clone();
        // Warm the UI from cache immediately, then refresh in the background.
        for t in [AssetType::Hdris, AssetType::Textures] {
            if let Some(cached) = crate::cache::load(t.cache_name()) {
                s.assets_by_type.insert(t, AssetListState::Loaded(cached));
            }
        }
        s.rebuild_prod_folder_cache();
        s
    }

    pub fn refresh(&mut self, t: AssetType) {
        if self.refreshing.contains(&t) {
            return;
        }
        if self.config.notion_token.is_empty() {
            self.assets_by_type.insert(
                t,
                AssetListState::Error("No Notion token configured".into()),
            );
            return;
        }
        // If we already have data, keep showing it while the background fetch runs.
        // Only show the "Loading…" placeholder when there's nothing to display yet.
        if !matches!(self.assets_by_type.get(&t), Some(AssetListState::Loaded(_))) {
            self.assets_by_type.insert(t, AssetListState::Loading);
        }
        self.refreshing.insert(t);
        let (tx, rx) = channel();
        let token = self.config.notion_token.clone();
        let db = t.db_id().to_string();
        thread::spawn(move || {
            let res = crate::notion::fetch_database(&token, &db).map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.notion_rx.insert(t, rx);
    }

    pub fn refresh_all_asset_types(&mut self) {
        self.refresh_published_assets();
        if self.config.notion_token.is_empty() {
            return;
        }
        for t in AssetType::all() {
            self.refresh(*t);
        }
    }

    /// Rebuild the full prod-folder existence cache from disk.
    /// Call when Notion data loads, window gains focus, or the asset list changes.
    pub fn rebuild_prod_folder_cache(&mut self) {
        self.prod_folder_cache.clear();
        for &t in AssetType::all() {
            let prod_root = self.prod_root_for(t);
            if let Some(AssetListState::Loaded(list)) = self.assets_by_type.get(&t) {
                for asset in &list.assets {
                    let key = RowKey {
                        asset_type: t,
                        slug: asset.slug.clone(),
                    };
                    self.prod_folder_cache
                        .insert(key, prod_root.join(&asset.slug).is_dir());
                }
            }
        }
    }

    /// Update the cache for a single asset (after a job finishes or a folder is created).
    pub fn update_prod_folder_cache_for(&mut self, key: &RowKey) {
        let exists = self.prod_root_for(key.asset_type).join(&key.slug).is_dir();
        self.prod_folder_cache.insert(key.clone(), exists);
    }

    pub fn refresh_published_assets(&mut self) {
        if self.refreshing_published {
            return;
        }
        self.refreshing_published = true;
        let (tx, rx) = channel();
        thread::spawn(move || {
            let res = crate::polyhaven::fetch_published_assets().map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.published_rx = Some(rx);
    }

    /// Drain Notion + job channels each frame.
    pub fn pump(&mut self) {
        let cursor_guard = self
            .cursor_moved_in_table_at
            .map(|t| t.elapsed().as_secs_f32() < 2.0)
            .unwrap_or(false);

        let types: Vec<_> = self.notion_rx.keys().copied().collect();
        for t in types {
            let res_opt = self.notion_rx.get(&t).and_then(|rx| rx.try_recv().ok());
            if let Some(res) = res_opt {
                self.notion_rx.remove(&t);
                self.refreshing.remove(&t);
                match res {
                    Ok(list) => {
                        if cursor_guard {
                            // Buffer — apply once the cursor has been idle for 2s.
                            self.pending_notion.insert(t, list);
                        } else {
                            let _ = crate::cache::save(t.cache_name(), &list);
                            self.assets_by_type.insert(t, AssetListState::Loaded(list));
                            self.rebuild_prod_folder_cache();
                        }
                    }
                    Err(msg) => {
                        self.assets_by_type.insert(t, AssetListState::Error(msg));
                    }
                }
            }
        }

        // Flush buffered updates once the cursor has been still for 2s.
        if !cursor_guard && !self.pending_notion.is_empty() {
            let pending: Vec<_> = self.pending_notion.drain().collect();
            for (t, list) in pending {
                let _ = crate::cache::save(t.cache_name(), &list);
                self.assets_by_type.insert(t, AssetListState::Loaded(list));
            }
            self.rebuild_prod_folder_cache();

            if let Some(res) = self.published_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.published_rx = None;
                self.refreshing_published = false;
                match res {
                    Ok(assets) => {
                        let _ = crate::cache::save(crate::polyhaven::cache_name(), &assets);
                        self.published_assets = assets;
                    }
                    Err(msg) => {
                        self.error_banner = Some(format!("Published asset refresh failed: {msg}"));
                    }
                }
            }
        }

        let status_keys: Vec<RowKey> = self.status_updates.keys().cloned().collect();
        for key in status_keys {
            let res_opt = self
                .status_updates
                .get(&key)
                .and_then(|job| job.rx.try_recv().ok());
            if let Some(res) = res_opt {
                let Some(job) = self.status_updates.remove(&key) else {
                    continue;
                };
                match res {
                    Ok(()) => {
                        if let Some(AssetListState::Loaded(list)) =
                            self.assets_by_type.get(&key.asset_type)
                        {
                            let _ = crate::cache::save(key.asset_type.cache_name(), list);
                        }
                    }
                    Err(msg) => {
                        set_asset_status(self, &key, job.previous);
                        self.error_banner =
                            Some(format!("Status update failed for {}: {msg}", key.slug));
                    }
                }
            }
        }

        let keys: Vec<RowKey> = self.jobs.keys().cloned().collect();
        for k in keys {
            let mut done = false;
            let mut finished_successfully = false;
            let mut err_msg: Option<String> = None;
            if let Some(job) = self.jobs.get(&k) {
                while let Ok(msg) = job.rx.try_recv() {
                    match msg {
                        JobMsg::FileDone { .. } => {}
                        JobMsg::FileFailed { rel_path, error } => {
                            err_msg = Some(format!("{}: {rel_path} — {error}", k.slug));
                            done = true;
                        }
                        JobMsg::Finished => {
                            done = true;
                            finished_successfully = true;
                        }
                        JobMsg::Cancelled => {
                            done = true;
                        }
                    }
                }
            }
            if let Some(m) = err_msg {
                self.error_banner = Some(m);
            }
            if done {
                if let Some(job) = self.jobs.remove(&k) {
                    if finished_successfully {
                        self.update_prod_folder_cache_for(&k);
                        if job.direction == Direction::Push {
                            self.start_push_verification(k.clone(), job.plan.clone());
                        }
                        let action = match job.direction {
                            Direction::Pull => "Pulled from prod",
                            Direction::Push => "Pushed to prod",
                        };
                        self.row_toasts.insert(
                            k,
                            RowToast {
                                text: format!(
                                    "{action} in {}",
                                    fmt_duration(job.started_at.elapsed())
                                ),
                                created_at: Instant::now(),
                            },
                        );
                    }
                }
            }
        }
        let verification_keys: Vec<RowKey> = self.verifications.keys().cloned().collect();
        for k in verification_keys {
            let mut done = false;
            let mut failure: Option<PendingVerificationFailure> = None;
            if let Some(job) = self.verifications.get(&k) {
                while let Ok(msg) = job.rx.try_recv() {
                    match msg {
                        VerifyMsg::FileDone { .. } => {}
                        VerifyMsg::Finished => {
                            done = true;
                        }
                        VerifyMsg::FileFailed { rel_path, error } => {
                            failure = Some(PendingVerificationFailure {
                                key: k.clone(),
                                rel_path,
                                error,
                            });
                            done = true;
                        }
                    }
                }
            }
            if done {
                self.verifications.remove(&k);
            }
            if failure.is_some() {
                self.pending_verification_failure = failure;
            }
        }
        self.row_toasts
            .retain(|_, toast| toast.created_at.elapsed() < Duration::from_secs(5));
    }

    pub fn local_root_for(&self, t: AssetType) -> PathBuf {
        self.config.local_root.join(t.folder())
    }
    pub fn prod_root_for(&self, t: AssetType) -> PathBuf {
        self.config.prod_root.join(t.folder())
    }

    fn start_push_verification(&mut self, key: RowKey, plan: crate::copy::plan::Plan) {
        let (tx, rx) = std::sync::mpsc::channel();
        let progress = Arc::new(JobProgress::default());
        crate::copy::job::spawn_verification(plan.files, progress.clone(), tx);
        self.verifications
            .insert(key, VerificationJob { progress, rx });
    }
}

pub fn start_job(state: &mut AppState, key: &RowKey, direction: Direction) {
    let (src_root, dst_root) = match direction {
        Direction::Pull => (
            state.prod_root_for(key.asset_type).join(&key.slug),
            state.local_root_for(key.asset_type).join(&key.slug),
        ),
        Direction::Push => (
            state.local_root_for(key.asset_type).join(&key.slug),
            state.prod_root_for(key.asset_type).join(&key.slug),
        ),
    };

    let plan = match build_plan(direction, &src_root, &dst_root) {
        Ok(p) => p,
        Err(e) => {
            state.error_banner = Some(format!("Plan failed: {e}"));
            return;
        }
    };

    if !plan.conflicts().is_empty() {
        state.pending_conflict = Some(PendingConflict {
            key: key.clone(),
            direction,
            plan,
        });
        return;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan.clone(), progress.clone(), tx);
    state.jobs.insert(
        key.clone(),
        RowJob {
            direction,
            plan,
            progress,
            rx,
            message: Arc::new(Mutex::new(String::new())),
            started_at: Instant::now(),
        },
    );
}

pub fn start_status_update(
    state: &mut AppState,
    key: &RowKey,
    page_id: &str,
    requested: StatusOption,
) {
    if state.status_updates.contains_key(key) {
        return;
    }
    let previous = set_asset_status(state, key, Some(status_from_option(&requested)));
    let (tx, rx) = std::sync::mpsc::channel();
    let token = state.config.notion_token.clone();
    let page_id = page_id.to_string();
    let requested_for_thread = requested.clone();
    thread::spawn(move || {
        let res = crate::notion::update_page_status(&token, &page_id, &requested_for_thread)
            .map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    state.status_updates.insert(
        key.clone(),
        StatusUpdateJob {
            rx,
            previous,
            requested,
        },
    );
}

fn status_from_option(option: &StatusOption) -> AssetStatus {
    AssetStatus {
        id: option.id.clone(),
        name: option.name.clone(),
        color: option.color.clone(),
        group: option.group,
    }
}

fn set_asset_status(
    state: &mut AppState,
    key: &RowKey,
    status: Option<AssetStatus>,
) -> Option<AssetStatus> {
    let Some(AssetListState::Loaded(list)) = state.assets_by_type.get_mut(&key.asset_type) else {
        return None;
    };
    let Some(asset) = list.assets.iter_mut().find(|asset| asset.slug == key.slug) else {
        return None;
    };
    let previous = asset.status.clone();
    asset.status = status;
    previous
}

pub fn execute_after_conflict(state: &mut AppState, choice: ConflictChoice) {
    let Some(pc) = state.pending_conflict.take() else {
        return;
    };
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
            plan.files
                .retain(|f| !matches!(f.action, Action::Conflict { .. }));
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan.clone(), progress.clone(), tx);
    state.jobs.insert(
        pc.key,
        RowJob {
            direction: pc.direction,
            plan,
            progress,
            rx,
            message: Arc::new(Mutex::new(String::new())),
            started_at: Instant::now(),
        },
    );
}

pub fn create_prod_folder(state: &mut AppState, key: &RowKey) {
    if !crate::slug::is_valid(&key.slug) {
        state.error_banner = Some("Cannot create Prod folder: slug has invalid characters".into());
        return;
    }
    let root = state.prod_root_for(key.asset_type).join(&key.slug);
    if let Err(err) = create_prod_folder_structure_at(&root, key.asset_type) {
        state.error_banner = Some(format!(
            "Could not create Prod folder for {}: {err}",
            key.slug
        ));
        return;
    }
    state.update_prod_folder_cache_for(key);
    let _ = open::that(root);
}

fn create_prod_folder_structure_at(
    root: &std::path::Path,
    asset_type: AssetType,
) -> std::io::Result<()> {
    let primary = match asset_type {
        AssetType::Hdris | AssetType::Textures => "raw",
    };
    for subfolder in [primary, "staging", "work"] {
        std::fs::create_dir_all(root.join(subfolder))?;
    }
    Ok(())
}

fn fmt_duration(duration: Duration) -> String {
    let secs = duration.as_secs().max(1);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn format_active_file_action(
    key: &RowKey,
    direction: Direction,
    current_file: Option<&str>,
) -> String {
    let verb = match direction {
        Direction::Pull => "Downloading",
        Direction::Push => "Uploading",
    };
    let target = match current_file {
        Some(file) if !file.is_empty() => {
            format!("{}/{}/{}", key.asset_type.folder(), key.slug, file)
        }
        _ => format!("{}/{}", key.asset_type.folder(), key.slug),
    };
    let suffix = match direction {
        Direction::Pull => "from Prod",
        Direction::Push => "to Prod",
    };
    format!("{verb} {target} {suffix}")
}

fn active_file_action_status(state: &AppState) -> Option<String> {
    if let Some(status) = state
        .jobs
        .iter()
        .min_by(|(a, _), (b, _)| {
            a.asset_type
                .order()
                .cmp(&b.asset_type.order())
                .then_with(|| a.slug.cmp(&b.slug))
        })
        .map(|(key, job)| {
            let current_file = job
                .progress
                .current_file
                .lock()
                .ok()
                .and_then(|file| file.clone());
            format_active_file_action(key, job.direction, current_file.as_deref())
        })
    {
        return Some(status);
    }

    state
        .verifications
        .iter()
        .min_by(|(a, _), (b, _)| {
            a.asset_type
                .order()
                .cmp(&b.asset_type.order())
                .then_with(|| a.slug.cmp(&b.slug))
        })
        .map(|(key, job)| {
            let current_file = job
                .progress
                .current_file
                .lock()
                .ok()
                .and_then(|file| file.clone());
            let target = match current_file.as_deref() {
                Some(file) if !file.is_empty() => {
                    format!("{}/{}/{}", key.asset_type.folder(), key.slug, file)
                }
                _ => format!("{}/{}", key.asset_type.folder(), key.slug),
            };
            format!("Verifying {target}")
        })
}

fn draw_status_bar(state: &AppState, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(4.0, 2.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let status = active_file_action_status(state).unwrap_or_default();
                    ui.label(egui::RichText::new(status).color(colors::TEXT_DISABLED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .color(colors::TEXT_DISABLED),
                        );
                    });
                });
            });
    });
}

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let gained_focus = ctx.input(|i| state.focus_refresh.update(i.focused));
    if gained_focus {
        state.refresh_all_asset_types();
        state.rebuild_prod_folder_cache();
    }

    egui::TopBottomPanel::top("menu").show(ctx, |ui| {
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(0.0, 4.0))
            .show(ui, |ui| menu::draw(state, ui));
    });
    if let Some(err) = state.error_banner.clone() {
        egui::TopBottomPanel::top("banner").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(colors::ERROR_BANNER, err);
                let tex = x_icon_texture(ui.ctx());
                let resp = ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(14.0, 14.0),
                    ))
                    .tint(egui::Color32::WHITE)
                    .sense(egui::Sense::click()),
                );
                if resp
                    .on_hover_text("Dismiss")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    state.error_banner = None;
                }
            });
        });
    }
    dialogs::token_prompt(state, ctx);
    dialogs::draw(state, ctx);
    draw_create_prod_folder_prompt(state, ctx);
    draw_verification_failure_prompt(state, ctx);
    draw_status_bar(state, ctx);
    let table_resp = egui::CentralPanel::default().show(ctx, |ui| table::draw(state, ui));

    // Track cursor movement inside the table panel for the 2s safety guard.
    let table_rect = table_resp.response.rect;
    ctx.input(|i| {
        if let Some(pos) = i.pointer.latest_pos() {
            if table_rect.contains(pos) && i.pointer.delta() != egui::Vec2::ZERO {
                state.cursor_moved_in_table_at = Some(Instant::now());
            }
        }
    });

    // Keep repainting while a pending update is waiting to be flushed.
    if !state.pending_notion.is_empty()
        || !state.row_toasts.is_empty()
        || !state.jobs.is_empty()
        || !state.verifications.is_empty()
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

fn draw_verification_failure_prompt(state: &mut AppState, ctx: &egui::Context) {
    let Some(failure) = state.pending_verification_failure.as_ref() else {
        return;
    };
    let slug = failure.key.slug.clone();
    let rel_path = failure.rel_path.clone();
    let error = failure.error.clone();
    let key = failure.key.clone();
    let mut retry = false;
    let mut ignore = false;

    egui::Window::new(format!("Copy verification failed — {slug}"))
        .collapsible(false)
        .resizable(false)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("The push finished, but post-copy verification found a mismatch.");
            ui.add_space(6.0);
            ui.label("Problem file:");
            ui.monospace(&rel_path);
            ui.add_space(4.0);
            ui.label("Verification error:");
            ui.monospace(&error);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Try again").clicked() {
                    retry = true;
                }
                if ui.button("Ignore").clicked() {
                    ignore = true;
                }
            });
        });

    if retry {
        state.pending_verification_failure = None;
        let failed_dst = state
            .prod_root_for(key.asset_type)
            .join(&key.slug)
            .join(&rel_path);
        if let Err(err) = std::fs::remove_file(&failed_dst) {
            if err.kind() != std::io::ErrorKind::NotFound {
                state.error_banner = Some(format!(
                    "Could not remove failed copy before retrying {}: {err}",
                    failed_dst.display()
                ));
                return;
            }
        }
        start_job(state, &key, Direction::Push);
    } else if ignore {
        state.pending_verification_failure = None;
    }
}

fn draw_create_prod_folder_prompt(state: &mut AppState, ctx: &egui::Context) {
    let Some(key) = state.pending_prod_folder_create.clone() else {
        return;
    };
    egui::Window::new("Create Prod folder?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!(
                "Do you want to create the folder for {}?",
                key.slug
            ));
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    create_prod_folder(state, &key);
                    state.pending_prod_folder_create = None;
                }
                if ui.button("Cancel").clicked() {
                    state.pending_prod_folder_create = None;
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use crate::copy::plan::Direction;

    #[test]
    fn prod_folder_structure_creates_expected_subfolders() {
        let temp = tempfile::tempdir().unwrap();

        super::create_prod_folder_structure_at(temp.path(), super::AssetType::Hdris).unwrap();

        assert!(temp.path().join("raw").is_dir());
        assert!(temp.path().join("staging").is_dir());
        assert!(temp.path().join("work").is_dir());
    }

    #[test]
    fn active_file_action_status_includes_direction_asset_and_file() {
        let key = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "foo".into(),
        };

        assert_eq!(
            super::format_active_file_action(&key, Direction::Pull, Some("bar.xyz")),
            "Downloading HDRIs/foo/bar.xyz from Prod"
        );
        assert_eq!(
            super::format_active_file_action(&key, Direction::Push, Some("bar.xyz")),
            "Uploading HDRIs/foo/bar.xyz to Prod"
        );
    }
}

pub fn notion_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/notion.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "notion_logo", "notion.svg"))
        .clone()
}

pub fn pull_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/box-arrow-in-down.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "pull_icon", "box-arrow-in-down.svg"))
        .clone()
}

pub fn push_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/cloud-upload-fill.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "push_icon", "cloud-upload-fill.svg"))
        .clone()
}

pub fn info_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/info.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_info", "info.svg"))
        .clone()
}

pub fn warn_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/exclamation-triangle.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_warn", "exclamation-triangle.svg"))
        .clone()
}

pub fn error_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/exclamation-diamond.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_error", "exclamation-diamond.svg"))
        .clone()
}

pub fn question_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/question.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "icon_question", "question.svg"))
        .clone()
}

pub fn loading_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/loading.png");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| {
        let image = egui_extras::image::load_image_bytes(BYTES).expect("loading.png");
        ctx.load_texture("loading_spinner", image, egui::TextureOptions::LINEAR)
    })
    .clone()
}

pub fn chevron_down_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/chevron-down.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "chevron_down", "chevron-down.svg"))
        .clone()
}

pub fn x_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/x.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "x_icon", "x.svg"))
        .clone()
}

pub fn external_link_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/box-arrow-up-right.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "external_link", "box-arrow-up-right.svg"))
        .clone()
}

pub fn check_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/check.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "check_icon", "check.svg"))
        .clone()
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
