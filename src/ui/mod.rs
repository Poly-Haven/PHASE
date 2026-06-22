mod asset_types;
mod authors;
pub mod colors;
mod dialogs;
mod file_watcher;
mod focus_refresh;
mod group_selector;
mod jobs;
pub mod layout;
mod loading_indicator;
mod menu;
mod scripts;
mod status_groups;
mod table;
mod textures;
mod thumbnails;

pub use textures::*;

pub use jobs::{
    create_prod_folder, execute_after_conflict, start_archive, start_job, start_status_update,
    start_title_rename, start_unarchive,
};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::auth::{AuthTokens, BrowserLogin, LoggedInIdentity};
use crate::config::Config;
use crate::copy::job::{JobMsg, JobProgress, VerifyMsg};
use crate::copy::plan::{build_plan_with_pull_filter, Action, Direction, Plan, PullFilterMode};
use crate::notion::{Asset, AssetList, AssetStatus, StatusOption};

const VERSION_NOTICE_DURATION: Duration = Duration::from_secs(10);

struct VersionNotice {
    message: String,
    expires_at: Instant,
}

struct UpdateCheckJob {
    rx: Receiver<Result<Option<crate::updater::UpdateInfo>, String>>,
    show_latest_notice_on_none: bool,
}

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
    pub fn api_type(self) -> &'static str {
        match self {
            Self::Hdris => "hdris",
            Self::Textures => "textures",
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

/// What kind of transfer a copy job is. Archive (Prod -> archive) and Unarchive
/// (archive -> Prod) flow through the same plan/copy/verify pipeline as Push and
/// Pull; they differ only in their roots, progress colour/direction, and
/// post-copy steps (Archive deletes Prod afterwards).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TransferKind {
    Push,
    Pull,
    Archive,
    Unarchive,
}

impl TransferKind {
    /// Present-progressive verb for the in-flight row label / status bar.
    pub fn progressive(self) -> &'static str {
        match self {
            TransferKind::Push => "Pushing",
            TransferKind::Pull => "Pulling",
            TransferKind::Archive => "Archiving",
            TransferKind::Unarchive => "Unarchiving",
        }
    }

    /// Past-tense label for the success toast.
    pub fn done_label(self) -> &'static str {
        match self {
            TransferKind::Push => "Pushed to prod",
            TransferKind::Pull => "Pulled from prod",
            TransferKind::Archive => "Archived",
            TransferKind::Unarchive => "Unarchived",
        }
    }

    pub fn touches_archive(self) -> bool {
        matches!(self, TransferKind::Archive | TransferKind::Unarchive)
    }
}

pub struct RowJob {
    pub kind: TransferKind,
    pub plan: crate::copy::plan::Plan,
    pub progress: Arc<JobProgress>,
    pub rx: Receiver<JobMsg>,
    pub started_at: Instant,
    #[allow(dead_code)]
    pub message: Arc<Mutex<String>>,
}

pub struct VerificationJob {
    /// The kind whose copy produced this verification (drives the small
    /// verify-bar colour). Today only `Push` runs a separate verify pass.
    pub kind: TransferKind,
    pub progress: Arc<JobProgress>,
    pub rx: Receiver<VerifyMsg>,
}

/// After an Archive copy fully verifies (inline), the asset's Prod folder is
/// deleted on a background thread (guarded by a re-scan). This tracks that final
/// step so the success toast reflects the whole archive duration.
pub struct ArchiveDelete {
    pub started_at: Instant,
    pub rx: Receiver<Result<(), String>>,
}

pub struct PendingVerificationFailure {
    pub key: RowKey,
    pub rel_path: String,
    pub error: String,
}

pub struct TransferFileListDialog {
    pub key: RowKey,
    pub direction: Direction,
    pub ignore_raws_tiffs: bool,
    pub plan: Option<Plan>,
    pub error: Option<String>,
    pub loading: bool,
    pub rx: Option<Receiver<Result<Plan, String>>>,
}

pub struct RowToast {
    pub text: String,
    pub created_at: Instant,
}

pub struct ThumbnailPreview {
    #[cfg(test)]
    pub signature: thumbnails::ThumbnailSignature,
    pub texture: egui::TextureHandle,
}

pub struct PendingConflict {
    pub key: RowKey,
    pub kind: TransferKind,
    pub plan: crate::copy::plan::Plan,
}

pub struct StatusUpdateJob {
    pub rx: Receiver<Result<Option<AuthTokens>, String>>,
    pub previous: Option<AssetStatus>,
    #[allow(dead_code)]
    pub requested: StatusOption,
}

pub struct TitleRenameJob {
    pub rx: Receiver<Result<Option<AuthTokens>, String>>,
    /// The new title the rename was requested to set.
    pub new_title: String,
}

pub struct UpdateInstallJob {
    pub rx: Receiver<Result<(), String>>,
}

pub struct ValidationJob {
    pub rx: Receiver<crate::validation::Msg>,
}

/// A background thread is building the copy plan for this asset.
pub struct PlanJob {
    pub kind: TransferKind,
    pub rx: Receiver<Result<Plan, String>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TransferAction {
    PushAll,
    PushStagingOnly,
    PullDefault,
    PullStagingOnly,
    PullAll,
}

impl TransferAction {
    pub const fn default_push() -> Self {
        Self::PushAll
    }

    pub const fn default_pull() -> Self {
        Self::PullDefault
    }

    pub const fn all() -> [Self; 5] {
        [
            Self::PushAll,
            Self::PushStagingOnly,
            Self::PullDefault,
            Self::PullStagingOnly,
            Self::PullAll,
        ]
    }

    pub const fn direction(self) -> Direction {
        match self {
            Self::PushAll | Self::PushStagingOnly => Direction::Push,
            Self::PullDefault | Self::PullStagingOnly | Self::PullAll => Direction::Pull,
        }
    }

    pub const fn kind(self) -> TransferKind {
        match self {
            Self::PushAll | Self::PushStagingOnly => TransferKind::Push,
            Self::PullDefault | Self::PullStagingOnly | Self::PullAll => TransferKind::Pull,
        }
    }

    pub const fn menu_label(self) -> &'static str {
        match self {
            Self::PushAll => "Push all files",
            Self::PushStagingOnly => "Push staging only",
            Self::PullDefault => "Pull without raws/tiffs",
            Self::PullStagingOnly => "Pull staging only",
            Self::PullAll => "Pull all files",
        }
    }

    pub const fn default_suffix(self) -> &'static str {
        match self {
            Self::PushAll => " (default)",
            Self::PullDefault => " (default)",
            _ => "",
        }
    }

    pub const fn nothing_label(self) -> &'static str {
        match self.direction() {
            Direction::Push => "nothing to push",
            Direction::Pull => "nothing to pull",
        }
    }
}

/// Summary of an action plan: how many files will be copied and their total size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionPreview {
    pub file_count: usize,
    pub bytes: u64,
}

pub struct TransferEstimateJob {
    pub rx: Receiver<Result<ActionPreview, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferFileDisplayRow {
    pub path: String,
    pub reason: &'static str,
    pub color: egui::Color32,
}

pub enum AuthMsg {
    Started(BrowserLogin),
    Success(AuthTokens),
    Error(String),
}

pub(super) fn transfer_file_display_rows(plan: &Plan) -> Vec<TransferFileDisplayRow> {
    plan.files
        .iter()
        .filter_map(|file| {
            let (reason, color) = match file.action {
                crate::copy::plan::Action::New => ("New file", crate::ui::colors::STATUS_COMPLETE),
                crate::copy::plan::Action::Overwrite => {
                    ("File updated", crate::ui::colors::MSG_INFO)
                }
                crate::copy::plan::Action::Conflict { dest_newer: true } => {
                    ("Conflict, destination newer", crate::ui::colors::MSG_ERROR)
                }
                crate::copy::plan::Action::Conflict { dest_newer: false } => {
                    ("Conflict, source newer", crate::ui::colors::MSG_ERROR)
                }
                crate::copy::plan::Action::Identical => return None,
            };
            Some(TransferFileDisplayRow {
                path: file.rel_path.display().to_string(),
                reason,
                color,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisibleValidationScope {
    pub keys: Vec<RowKey>,
    selected_types: Vec<AssetType>,
    selected_status_groups: Vec<crate::notion::StatusGroup>,
    author_filters: Vec<String>,
    search_query: String,
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
    pub author_filters: Vec<String>,
    pub assets_by_type: HashMap<AssetType, AssetListState>,
    pub error_banner: Option<String>,
    pub jobs: HashMap<RowKey, RowJob>,
    pub plan_jobs: HashMap<RowKey, PlanJob>,
    pub verifications: HashMap<RowKey, VerificationJob>,
    /// In-flight post-archive Prod deletions (keyed by asset).
    pub archive_deletes: HashMap<RowKey, ArchiveDelete>,
    pub status_updates: HashMap<RowKey, StatusUpdateJob>,
    pub title_renames: HashMap<RowKey, TitleRenameJob>,
    pub notion_rx: HashMap<AssetType, Receiver<Result<(AssetList, Option<AuthTokens>), String>>>,
    pub pending_conflict: Option<PendingConflict>,
    pub pending_verification_failure: Option<PendingVerificationFailure>,
    pub transfer_file_list_dialog: Option<TransferFileListDialog>,
    pub pending_prod_folder_create: Option<RowKey>,
    pub pending_local_folder_delete: Option<RowKey>,
    /// Asset awaiting archive confirmation (set by the menu item / info message).
    pub pending_archive: Option<RowKey>,
    pub row_toasts: HashMap<RowKey, RowToast>,
    pub published_assets: crate::polyhaven::PublishedAssets,
    pub published_rx: Option<Receiver<Result<crate::polyhaven::PublishedAssets, String>>>,
    pub refreshing_published: bool,
    pub token_prompt_open: bool,
    pub token_input: String,
    pub auth_login: Option<BrowserLogin>,
    pub auth_rx: Option<Receiver<AuthMsg>>,
    pub logged_in_identity: Option<LoggedInIdentity>,
    pub settings_open: bool,
    pub settings_local_root_input: String,
    pub settings_affinity_path_input: String,
    pub settings_open_notion_links_in_desktop_app: bool,
    /// Asset types whose background fetch is currently in flight.
    pub refreshing: HashSet<AssetType>,
    /// Asset API results buffered while the cursor is active in the table.
    pub pending_notion: HashMap<AssetType, AssetList>,
    /// Last time the pointer moved while inside the table area.
    pub cursor_moved_in_table_at: Option<Instant>,
    pub focus_refresh: focus_refresh::State,
    /// Cached result of `is_dir()` for each asset's prod folder.
    /// Rebuilt when asset API data loads, window gains focus, a job finishes,
    /// or a prod folder is created — never on every frame.
    pub prod_folder_cache: HashMap<RowKey, bool>,
    /// Cached result of `is_dir()` for each asset's archive folder. Rebuilt
    /// alongside `prod_folder_cache`. Used to suppress the "No prod folder"
    /// warning and to drive the "Published. Archive files?" message.
    pub archive_folder_cache: HashMap<RowKey, bool>,
    /// Receiver for an in-flight background rebuild of the prod + archive folder
    /// existence caches (both checked on the same thread to avoid duplicate
    /// network round-trips).
    pub prod_cache_rx: Option<Receiver<(HashMap<RowKey, bool>, HashMap<RowKey, bool>)>>,
    pub thumbnail_cache_root: PathBuf,
    pub thumbnail_revisions: HashMap<RowKey, Arc<AtomicU64>>,
    pub thumbnail_jobs: HashMap<RowKey, thumbnails::ThumbnailJob>,
    pub thumbnail_previews: HashMap<RowKey, ThumbnailPreview>,
    pub thumbnail_cleanup_rx: Option<Receiver<Result<usize, String>>>,
    pub author_avatar_textures: HashMap<String, egui::TextureHandle>,
    /// Cached result of `is_dir()` for each asset's local working folder.
    /// Rebuilt synchronously (local disk) on focus gain and after pulls.
    pub local_folder_cache: HashMap<RowKey, bool>,
    pub dismissed_warning_keys: HashSet<String>,
    pub validation_results: HashMap<RowKey, Vec<crate::validation::Finding>>,
    pub validation_job: Option<ValidationJob>,
    pub visible_validation_scope: VisibleValidationScope,
    update_check: Option<UpdateCheckJob>,
    pub pending_update: Option<crate::updater::UpdateInfo>,
    version_notice: Option<VersionNotice>,
    pub update_dialog_open: bool,
    pub update_install: Option<UpdateInstallJob>,
    pub transfer_estimates: HashMap<(RowKey, TransferAction), ActionPreview>,
    pub transfer_estimate_jobs: HashMap<(RowKey, TransferAction), TransferEstimateJob>,
    pub script_jobs: HashMap<scripts::ScriptKey, scripts::ScriptJob>,
    pub script_queue: VecDeque<scripts::QueuedScript>,
    pub script_results: HashMap<scripts::ScriptKey, scripts::ScriptRun>,
    pub script_output_dialog: Option<scripts::ScriptKey>,
    pub search_query: String,
    /// Filesystem watcher (lazily created once an egui Context is available).
    pub file_watcher: Option<file_watcher::FileWatcher>,
    /// Time of the last activity in the PHASE window (mouse move, key input, or
    /// focus change). Drives the activity-aware prod monitoring schedule.
    pub last_activity_at: Instant,
    /// Last observed window focus state, for detecting focus changes as activity.
    pub watcher_was_focused: bool,
    /// Mode selected on the previous frame, for detecting mode transitions.
    pub last_watch_mode: file_watcher::WatchMode,
    /// When the next poll tick is due (only meaningful in polling modes).
    pub next_poll_at: Instant,
    /// Set true whenever the desired watch set may have changed; reconcile only
    /// touches the filesystem/network when this is set, then clears it.
    pub watch_dirty: bool,
    /// Pending debounced re-validations keyed by asset, with cache-update flags.
    pub watch_pending: HashMap<RowKey, PendingWatch>,
    /// Keys awaiting (re-)validation while a validation job is already running.
    pub pending_validation_keys: HashSet<RowKey>,
}

/// A debounced re-validation request accumulated from filesystem events.
pub struct PendingWatch {
    /// Fire once `now >= deadline` (extended by each new event).
    pub deadline: Instant,
    /// Hard cap so a constant stream of writes cannot postpone validation forever.
    pub hard_deadline: Instant,
    pub update_local: bool,
    pub update_prod: bool,
}

fn initial_author_filters(config: &Config, selected_types: &[AssetType]) -> Vec<String> {
    if !config.last_author_filters_by_type.is_empty() {
        return author_filters_for_types(config, selected_types);
    }
    let mut filters = if !config.last_author_filters.is_empty() {
        config.last_author_filters.clone()
    } else if !config.last_author_filter.is_empty() {
        vec![config.last_author_filter.clone()]
    } else {
        Vec::new()
    };
    normalize_author_filters(&mut filters);
    filters
}

fn author_filters_for_types(config: &Config, selected_types: &[AssetType]) -> Vec<String> {
    let mut filters = Vec::new();
    for asset_type in selected_types {
        if let Some(saved) = config.last_author_filters_by_type.get(asset_type.label()) {
            filters.extend(saved.iter().cloned());
        }
    }
    normalize_author_filters(&mut filters);
    filters
}

fn normalize_author_filters(filters: &mut Vec<String>) {
    filters.retain(|filter| !filter.is_empty());
    filters.sort();
    filters.dedup();
}

pub(super) fn asset_author_source(asset: &Asset) -> String {
    if !asset.author_profiles.is_empty() {
        return asset
            .author_profiles
            .iter()
            .map(|author| author.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }

    if !asset.authors.is_empty() {
        return asset.authors.join(", ");
    }

    asset.author.clone()
}

fn current_update_check_day() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0)
}

fn thumbnail_cache_root() -> PathBuf {
    match crate::config::cache_dir() {
        Ok(dir) => dir.join("thumbnails"),
        Err(err) => {
            let fallback_root = dirs::data_local_dir()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from(r"C:\"));
            let path = fallback_root.join("phase").join("cache").join("thumbnails");
            log::warn!(
                "Using degraded thumbnail cache root {} after cache dir lookup failed: {err}",
                path.display()
            );
            path
        }
    }
}

fn should_check_for_update(last_check_day: Option<u64>, current_day: u64) -> bool {
    last_check_day != Some(current_day)
}

fn should_force_update_check(force: bool, last_check_day: Option<u64>, current_day: u64) -> bool {
    force || should_check_for_update(last_check_day, current_day)
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let selected_types = if config.last_asset_types.is_empty() {
            asset_types::from_labels(&[config.last_tab.clone()])
        } else {
            asset_types::from_labels(&config.last_asset_types)
        };
        let selected_status_groups = if config.last_selected_status_groups.is_empty() {
            crate::notion::StatusGroup::default_filter()
        } else {
            config.last_selected_status_groups.clone()
        };
        let current_type = selected_types.first().copied().unwrap_or(AssetType::Hdris);
        let author_filters = initial_author_filters(&config, &selected_types);
        let author_filter = author_filters.first().cloned().unwrap_or_default();

        let mut s = Self {
            current_type,
            selected_types,
            selected_status_groups,
            author_filter,
            author_filters,
            config,
            assets_by_type: HashMap::new(),
            error_banner: None,
            jobs: HashMap::new(),
            plan_jobs: HashMap::new(),
            verifications: HashMap::new(),
            archive_deletes: HashMap::new(),
            status_updates: HashMap::new(),
            title_renames: HashMap::new(),
            notion_rx: HashMap::new(),
            pending_conflict: None,
            pending_verification_failure: None,
            transfer_file_list_dialog: None,
            pending_prod_folder_create: None,
            pending_local_folder_delete: None,
            pending_archive: None,
            row_toasts: HashMap::new(),
            published_assets: crate::cache::load(crate::polyhaven::cache_name())
                .unwrap_or_default(),
            published_rx: None,
            refreshing_published: false,
            token_prompt_open: false,
            token_input: String::new(),
            auth_login: None,
            auth_rx: None,
            logged_in_identity: None,
            settings_open: false,
            settings_local_root_input: String::new(),
            settings_affinity_path_input: String::new(),
            settings_open_notion_links_in_desktop_app: false,
            refreshing: HashSet::new(),
            pending_notion: HashMap::new(),
            cursor_moved_in_table_at: None,
            focus_refresh: focus_refresh::State::default(),
            prod_folder_cache: HashMap::new(),
            archive_folder_cache: HashMap::new(),
            prod_cache_rx: None,
            thumbnail_cache_root: thumbnail_cache_root(),
            thumbnail_revisions: HashMap::new(),
            thumbnail_jobs: HashMap::new(),
            thumbnail_previews: HashMap::new(),
            thumbnail_cleanup_rx: None,
            author_avatar_textures: HashMap::new(),
            local_folder_cache: HashMap::new(),
            dismissed_warning_keys: crate::validation::load_dismissed_warning_keys()
                .unwrap_or_default(),
            validation_results: HashMap::new(),
            validation_job: None,
            visible_validation_scope: VisibleValidationScope::default(),
            update_check: None,
            pending_update: None,
            version_notice: None,
            update_dialog_open: false,
            update_install: None,
            transfer_estimates: HashMap::new(),
            transfer_estimate_jobs: HashMap::new(),
            script_jobs: HashMap::new(),
            script_queue: VecDeque::new(),
            script_results: HashMap::new(),
            script_output_dialog: None,
            search_query: String::new(),
            file_watcher: None,
            last_activity_at: Instant::now(),
            watcher_was_focused: false,
            last_watch_mode: file_watcher::WatchMode::RealTime,
            next_poll_at: Instant::now(),
            watch_dirty: true,
            watch_pending: HashMap::new(),
            pending_validation_keys: HashSet::new(),
        };
        s.token_prompt_open = !s.config.has_access_token() && !s.config.can_refresh_access_token();
        s.token_input.clear();
        s.settings_local_root_input = s.config.local_root.display().to_string();
        s.settings_affinity_path_input = s.config.affinity_path.display().to_string();
        s.settings_open_notion_links_in_desktop_app = s.config.open_notion_links_in_desktop_app;
        s.refresh_logged_in_identity();
        // Warm the UI from cache immediately, then refresh in the background.
        for t in [AssetType::Hdris, AssetType::Textures] {
            if let Some(cached) = crate::cache::load(t.cache_name()) {
                s.assets_by_type.insert(t, AssetListState::Loaded(cached));
            }
        }
        s.rebuild_prod_folder_cache();
        s.start_thumbnail_cleanup();
        s.start_update_check();
        s
    }

    pub fn apply_author_filters_for_selected_types(&mut self) {
        self.author_filters = author_filters_for_types(&self.config, &self.selected_types);
        self.author_filter = self.author_filters.first().cloned().unwrap_or_default();
    }

    pub fn persist_author_filters_for_selected_types(&mut self) {
        self.author_filter = self.author_filters.first().cloned().unwrap_or_default();
        self.config.last_author_filter = self.author_filter.clone();
        self.config.last_author_filters = self.author_filters.clone();
        for asset_type in &self.selected_types {
            let filters = self.author_filters_for_type(*asset_type);
            self.config
                .last_author_filters_by_type
                .insert(asset_type.label().to_string(), filters);
        }
    }

    fn author_filters_for_type(&self, asset_type: AssetType) -> Vec<String> {
        if self.author_filters.is_empty() {
            return Vec::new();
        }
        let Some(AssetListState::Loaded(list)) = self.assets_by_type.get(&asset_type) else {
            return self.author_filters.clone();
        };
        let available_sources: Vec<String> = list.assets.iter().map(asset_author_source).collect();
        let available =
            authors::filter_options(available_sources.iter().map(|source| source.as_str()));
        self.author_filters
            .iter()
            .filter(|filter| available.iter().any(|author| author == *filter))
            .cloned()
            .collect()
    }

    fn start_update_check_impl(&mut self, force: bool) {
        if let Some(job) = self.update_check.as_mut() {
            if force {
                job.show_latest_notice_on_none = true;
            }
            return;
        }
        let today = current_update_check_day();
        if !should_force_update_check(force, self.config.last_update_check_day, today) {
            return;
        }
        self.config.last_update_check_day = Some(today);
        let _ = crate::config::save(&self.config);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let res = crate::updater::check_for_update().map_err(|err| err.to_string());
            let _ = tx.send(res);
        });
        self.update_check = Some(UpdateCheckJob {
            rx,
            show_latest_notice_on_none: force,
        });
    }

    fn start_update_check(&mut self) {
        self.start_update_check_impl(false);
    }

    pub fn start_update_check_force(&mut self) {
        self.start_update_check_impl(true);
    }

    pub fn start_update_install(&mut self) {
        if self.update_install.is_some() {
            return;
        }
        let (tx, rx) = channel();
        thread::spawn(move || {
            let res = latest_update_tag_for_install(|| {
                crate::updater::check_for_update().map_err(|err| err.to_string())
            })
            .and_then(|tag| {
                crate::updater::install_update_and_restart(&tag).map_err(|err| err.to_string())
            });
            let _ = tx.send(res);
        });
        self.update_install = Some(UpdateInstallJob { rx });
    }

    fn clear_expired_version_notice(&mut self, now: Instant) {
        if self
            .version_notice
            .as_ref()
            .is_some_and(|notice| notice.expires_at <= now)
        {
            self.version_notice = None;
        }
    }

    fn set_version_notice(&mut self, message: impl Into<String>) {
        self.version_notice = Some(VersionNotice {
            message: message.into(),
            expires_at: Instant::now() + VERSION_NOTICE_DURATION,
        });
    }

    pub(super) fn transfer_plan_roots(
        &self,
        key: &RowKey,
        action: TransferAction,
    ) -> (PathBuf, PathBuf, PullFilterMode) {
        let local_root = self.local_root_for(key.asset_type).join(&key.slug);
        let prod_root = self.prod_root_for(key.asset_type).join(&key.slug);
        let local_staging = local_root.join("staging");
        let prod_staging = prod_root.join("staging");
        match action {
            TransferAction::PushAll => (local_root, prod_root, PullFilterMode::None),
            TransferAction::PushStagingOnly => (local_staging, prod_staging, PullFilterMode::None),
            TransferAction::PullDefault => {
                (prod_root, local_root, PullFilterMode::AlwaysSkipRawAndTif)
            }
            TransferAction::PullStagingOnly => (prod_staging, local_staging, PullFilterMode::None),
            TransferAction::PullAll => (prod_root, local_root, PullFilterMode::None),
        }
    }

    pub(super) fn build_transfer_plan(
        direction: Direction,
        src_root: PathBuf,
        dst_root: PathBuf,
        pull_filter: PullFilterMode,
    ) -> Result<Plan, String> {
        if !src_root.is_dir() {
            return Ok(Plan {
                direction,
                src_root,
                dst_root,
                files: Vec::new(),
                total_bytes_to_copy: 0,
            });
        }
        build_plan_with_pull_filter(direction, &src_root, &dst_root, pull_filter)
            .map_err(|e| e.to_string())
    }

    fn transfer_file_list_roots(
        &self,
        key: &RowKey,
        direction: Direction,
        ignore_raws_tiffs: bool,
    ) -> (PathBuf, PathBuf, PullFilterMode) {
        match direction {
            Direction::Push => (
                self.local_root_for(key.asset_type).join(&key.slug),
                self.prod_root_for(key.asset_type).join(&key.slug),
                PullFilterMode::None,
            ),
            Direction::Pull => (
                self.prod_root_for(key.asset_type).join(&key.slug),
                self.local_root_for(key.asset_type).join(&key.slug),
                if ignore_raws_tiffs {
                    PullFilterMode::AlwaysSkipRawAndTif
                } else {
                    PullFilterMode::None
                },
            ),
        }
    }

    pub(super) fn reload_transfer_file_list(&mut self) {
        let Some(dialog) = self.transfer_file_list_dialog.as_ref() else {
            return;
        };
        let key = dialog.key.clone();
        let direction = dialog.direction;
        let ignore_raws_tiffs = dialog.ignore_raws_tiffs;
        let (src_root, dst_root, pull_filter) =
            self.transfer_file_list_roots(&key, direction, ignore_raws_tiffs);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let result = Self::build_transfer_plan(direction, src_root, dst_root, pull_filter);
            let _ = tx.send(result);
        });
        let Some(dialog) = self.transfer_file_list_dialog.as_mut() else {
            return;
        };
        dialog.plan = None;
        dialog.error = None;
        dialog.loading = true;
        dialog.rx = Some(rx);
    }

    pub(super) fn open_transfer_file_list(&mut self, key: &RowKey, direction: Direction) {
        self.transfer_file_list_dialog = Some(TransferFileListDialog {
            key: key.clone(),
            direction,
            ignore_raws_tiffs: matches!(direction, Direction::Pull),
            plan: None,
            error: None,
            loading: false,
            rx: None,
        });
        self.reload_transfer_file_list();
    }

    pub fn start_transfer_estimate(&mut self, key: &RowKey, action: TransferAction, force: bool) {
        let estimate_key = (key.clone(), action);
        if (!force && self.transfer_estimates.contains_key(&estimate_key))
            || self.transfer_estimate_jobs.contains_key(&estimate_key)
            || self.plan_jobs.contains_key(key)
            || self.jobs.contains_key(key)
        {
            return;
        }
        let (src_root, dst_root, pull_filter) = self.transfer_plan_roots(key, action);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let result =
                Self::build_transfer_plan(action.direction(), src_root, dst_root, pull_filter).map(
                    |plan| {
                        let file_count = plan
                            .files
                            .iter()
                            .filter(|f| {
                                matches!(
                                    f.action,
                                    crate::copy::plan::Action::New
                                        | crate::copy::plan::Action::Overwrite
                                )
                            })
                            .count();
                        ActionPreview {
                            file_count,
                            bytes: plan.total_bytes_to_copy,
                        }
                    },
                );
            let _ = tx.send(result);
        });
        self.transfer_estimate_jobs
            .insert(estimate_key, TransferEstimateJob { rx });
    }

    /// Starts transfer estimates for all currently visible assets and transfer variants.
    /// Called on focus gain and when the visible scope changes.
    pub fn start_transfer_estimates_for_visible(&mut self, force: bool) {
        let keys = self.visible_asset_keys();
        for key in keys {
            for action in TransferAction::all() {
                self.start_transfer_estimate(&key, action, force);
            }
        }
    }

    pub fn clear_transfer_estimates_for_key(&mut self, key: &RowKey) {
        self.transfer_estimates
            .retain(|(estimate_key, _), _| estimate_key != key);
        self.transfer_estimate_jobs
            .retain(|(estimate_key, _), _| estimate_key != key);
    }

    pub fn refresh(&mut self, t: AssetType) {
        if self.refreshing.contains(&t) {
            return;
        }
        if !self.config.has_access_token() && !self.config.can_refresh_access_token() {
            self.assets_by_type.insert(
                t,
                AssetListState::Error("Authentication required: please log in".into()),
            );
            self.token_prompt_open = true;
            return;
        }
        // If we already have data, keep showing it while the background fetch runs.
        // Only show the "Loading…" placeholder when there's nothing to display yet.
        if !matches!(self.assets_by_type.get(&t), Some(AssetListState::Loaded(_))) {
            self.assets_by_type.insert(t, AssetListState::Loading);
        }
        self.refreshing.insert(t);
        let (tx, rx) = channel();
        let mut config = self.config.clone();
        let asset_type = t.api_type().to_string();
        thread::spawn(move || {
            let res = (|| {
                let before = (
                    config.auth_access_token.clone(),
                    config.auth_refresh_token.clone(),
                    config.auth_expires_at,
                );
                let token = crate::auth::ensure_access_token(&mut config)?;
                let tokens = if before
                    != (
                        config.auth_access_token.clone(),
                        config.auth_refresh_token.clone(),
                        config.auth_expires_at,
                    ) {
                    Some(AuthTokens {
                        access_token: config.auth_access_token.clone(),
                        refresh_token: config.auth_refresh_token.clone(),
                        expires_at: config.auth_expires_at,
                    })
                } else {
                    None
                };
                let list = crate::notion::fetch_assets(&token, &asset_type)?;
                Ok((list, tokens))
            })()
            .map_err(|e: anyhow::Error| e.to_string());
            let _ = tx.send(res);
        });
        self.notion_rx.insert(t, rx);
    }

    pub fn refresh_all_asset_types(&mut self) {
        self.refresh_published_assets();
        for t in AssetType::all() {
            self.refresh(*t);
        }
    }

    /// Kick off a background rebuild of the prod-folder and archive-folder
    /// existence caches. When the thread finishes, `pump()` swaps both in.
    pub fn rebuild_prod_folder_cache(&mut self) {
        let to_check: Vec<(RowKey, std::path::PathBuf, std::path::PathBuf)> = self
            .visible_asset_keys()
            .into_iter()
            .map(|key| {
                let prod = self.prod_root_for(key.asset_type).join(&key.slug);
                let archive = self.archive_root_for(key.asset_type).join(&key.slug);
                (key, prod, archive)
            })
            .collect();
        if to_check.is_empty() {
            self.prod_folder_cache.clear();
            self.archive_folder_cache.clear();
            return;
        }
        let (tx, rx) = channel();
        thread::spawn(move || {
            let mut prod = HashMap::with_capacity(to_check.len());
            let mut archive = HashMap::with_capacity(to_check.len());
            for (key, prod_path, archive_path) in to_check {
                prod.insert(key.clone(), prod_path.is_dir());
                archive.insert(key, archive_path.is_dir());
            }
            let _ = tx.send((prod, archive));
        });
        self.prod_cache_rx = Some(rx);
    }

    /// Synchronously rebuild the local-folder existence cache (local disk — fast).
    pub fn rebuild_local_folder_cache(&mut self) {
        self.local_folder_cache.clear();
        for key in self.visible_asset_keys() {
            let exists = self.local_root_for(key.asset_type).join(&key.slug).is_dir();
            self.local_folder_cache.insert(key, exists);
        }
    }

    pub fn start_thumbnail_cleanup(&mut self) {
        if self.thumbnail_cleanup_rx.is_some() {
            return;
        }
        let cache_root = self.thumbnail_cache_root.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let result = thumbnails::prune_thumbnail_cache(&cache_root);
            let _ = tx.send(result);
        });
        self.thumbnail_cleanup_rx = Some(rx);
    }

    fn thumbnail_revision_state(&mut self, key: &RowKey) -> Arc<AtomicU64> {
        self.thumbnail_revisions
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone()
    }

    fn prune_thumbnail_revisions_to_visible_scope(&mut self) {
        let visible: HashSet<RowKey> = self.visible_asset_keys().into_iter().collect();
        self.thumbnail_revisions.retain(|key, _| {
            visible.contains(key)
                || self.thumbnail_jobs.contains_key(key)
                || self.thumbnail_previews.contains_key(key)
        });
    }

    fn spawn_thumbnail_job_for_key(&mut self, key: &RowKey) {
        if self.thumbnail_jobs.contains_key(key) {
            return;
        }
        let revision_state = self.thumbnail_revision_state(key);
        let revision = revision_state.load(Ordering::Acquire);
        if revision == 0 {
            return;
        }
        let check_archive = self.asset_is_complete(key);
        let job = thumbnails::spawn_thumbnail_job(
            self.thumbnail_cache_root.clone(),
            self.config.local_root.clone(),
            self.config.prod_root.clone(),
            self.config.archive_root.clone(),
            key.clone(),
            check_archive,
            revision,
            revision_state,
        );
        self.thumbnail_jobs.insert(key.clone(), job);
    }

    pub fn start_thumbnail_refresh_for_keys<I>(&mut self, keys: I)
    where
        I: IntoIterator<Item = RowKey>,
    {
        let mut unique = HashSet::new();
        for key in keys {
            if !unique.insert(key.clone()) {
                continue;
            }
            let revision_state = self.thumbnail_revision_state(&key);
            revision_state.fetch_add(1, Ordering::AcqRel);
            self.spawn_thumbnail_job_for_key(&key);
        }
    }

    fn thumbnail_texture_name(key: &RowKey) -> String {
        format!("thumbnail:{}:{}", key.asset_type.order(), key.slug)
    }

    /// Update the local cache for a single asset after a pull finishes.
    pub fn update_local_folder_cache_for(&mut self, key: &RowKey) {
        let exists = self.local_root_for(key.asset_type).join(&key.slug).is_dir();
        self.local_folder_cache.insert(key.clone(), exists);
    }

    /// Update the cache for a single asset (after a job finishes or a folder is created).
    pub fn update_prod_folder_cache_for(&mut self, key: &RowKey) {
        let exists = self.prod_root_for(key.asset_type).join(&key.slug).is_dir();
        self.prod_folder_cache.insert(key.clone(), exists);
        self.watch_dirty = true;
    }

    /// Create the filesystem watcher once an egui Context is available. Called
    /// from the app update loop; tests construct `AppState` without a Context and
    /// therefore run without a watcher.
    pub fn ensure_file_watcher(&mut self, ctx: &egui::Context) {
        if self.file_watcher.is_none() {
            self.file_watcher = Some(file_watcher::FileWatcher::new(ctx.clone()));
            self.last_activity_at = Instant::now();
            self.watcher_was_focused = ctx.input(|i| i.focused);
            self.watch_dirty = true;
        }
    }

    /// Current activity-aware monitoring mode based on PHASE-window inactivity.
    fn watch_mode(&self) -> file_watcher::WatchMode {
        file_watcher::WatchMode::for_inactivity(self.last_activity_at.elapsed())
    }

    /// Visible asset keys that currently have a validation error and an existing
    /// prod slug folder — the set whose prod contents we monitor.
    fn error_keys_with_prod_folder(&self) -> Vec<RowKey> {
        let visible: HashSet<RowKey> = self.visible_asset_keys().into_iter().collect();
        self.validation_results
            .iter()
            .filter(|(key, findings)| {
                visible.contains(key)
                    && self.prod_folder_cache.get(*key) == Some(&true)
                    && findings
                        .iter()
                        .any(|finding| finding.severity == crate::validation::Severity::Error)
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Map a filesystem path to the asset it belongs to, by stripping the
    /// matching (case-insensitive) type-root prefix and taking the next
    /// component as the slug.
    fn key_for_path(
        &self,
        path: &std::path::Path,
        source: file_watcher::WatchSource,
    ) -> Option<RowKey> {
        for &asset_type in &self.selected_types {
            let root = match source {
                file_watcher::WatchSource::Local => self.local_root_for(asset_type),
                file_watcher::WatchSource::Prod => self.prod_root_for(asset_type),
            };
            if let Some(slug) = slug_under_root(&root, path) {
                return Some(RowKey { asset_type, slug });
            }
        }
        None
    }

    /// Re-validate `keys`, coalescing with any validation already running so
    /// watcher churn never drops an in-flight job's results.
    fn queue_revalidation(&mut self, keys: Vec<RowKey>) {
        if keys.is_empty() {
            return;
        }
        self.pending_validation_keys.extend(keys);
        if self.validation_job.is_none() {
            let keys: Vec<RowKey> = self.pending_validation_keys.drain().collect();
            self.start_validation_for_keys(keys);
        }
    }

    /// Reconcile the watch set with the current mode and error set. Only touches
    /// the filesystem/network when `watch_dirty` is set.
    fn reconcile_file_watcher(&mut self) {
        if self.file_watcher.is_none() || !self.watch_dirty {
            return;
        }
        self.watch_dirty = false;

        let local_roots: Vec<PathBuf> = self
            .selected_types
            .iter()
            .map(|t| self.local_root_for(*t))
            .collect();

        let mode = self.watch_mode();
        let prod_paths: Vec<(PathBuf, notify::RecursiveMode)> = if mode.is_real_time() {
            let mut paths = Vec::new();
            for &asset_type in &self.selected_types {
                paths.push((
                    self.prod_root_for(asset_type),
                    notify::RecursiveMode::NonRecursive,
                ));
            }
            for key in self.error_keys_with_prod_folder() {
                let slug = self.prod_root_for(key.asset_type).join(&key.slug);
                let staging = slug.join("staging");
                paths.push((slug, notify::RecursiveMode::NonRecursive));
                if staging.is_dir() {
                    paths.push((staging, notify::RecursiveMode::Recursive));
                }
            }
            paths
        } else {
            Vec::new()
        };

        log::debug!(
            "watcher reconcile: mode={mode:?} local_roots={} prod_paths={}",
            local_roots.len(),
            prod_paths.len()
        );
        if let Some(watcher) = self.file_watcher.as_mut() {
            watcher.ensure_local(&local_roots);
            watcher.set_prod(&prod_paths);
        }
    }

    /// Drain watcher events, apply debounced re-validation, run poll ticks, and
    /// reconcile the watch set. Called at the end of `pump()`.
    fn pump_file_watcher(&mut self) {
        if self.file_watcher.is_none() {
            return;
        }

        // 1. Drain raw events into the debounce map.
        let events = self
            .file_watcher
            .as_ref()
            .map(|watcher| watcher.drain())
            .unwrap_or_default();
        let now = Instant::now();
        for event in events {
            for path in &event.paths {
                let Some(key) = self.key_for_path(path, event.source) else {
                    continue;
                };
                log::debug!(
                    "watcher event: {:?} {} -> {}/{}",
                    event.source,
                    path.display(),
                    key.asset_type.folder(),
                    key.slug
                );
                let entry = self.watch_pending.entry(key).or_insert(PendingWatch {
                    deadline: now,
                    hard_deadline: now + Duration::from_secs(8),
                    update_local: false,
                    update_prod: false,
                });
                entry.deadline = now + Duration::from_millis(1500);
                match event.source {
                    file_watcher::WatchSource::Local => entry.update_local = true,
                    file_watcher::WatchSource::Prod => entry.update_prod = true,
                }
            }
            // A prod event may have created/removed `staging`; re-evaluate watches.
            if event.source == file_watcher::WatchSource::Prod {
                self.watch_dirty = true;
            }
        }

        // 2. Fire debounced re-validations whose deadline (or hard cap) elapsed.
        let now = Instant::now();
        let due: Vec<RowKey> = self
            .watch_pending
            .iter()
            .filter(|(_, pending)| now >= pending.deadline || now >= pending.hard_deadline)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &due {
            if let Some(pending) = self.watch_pending.remove(key) {
                if pending.update_local {
                    self.update_local_folder_cache_for(key);
                }
                if pending.update_prod {
                    self.update_prod_folder_cache_for(key);
                }
            }
        }
        let due_count = due.len();
        let thumbnail_due = due.clone();
        self.queue_revalidation(due);
        if due_count > 0 {
            self.start_thumbnail_refresh_for_keys(thumbnail_due);
        }
        if due_count > 0 {
            log::debug!("watcher re-validating {due_count} asset(s) after debounce");
        }

        // 3. Handle mode transitions and poll ticks.
        let mode = self.watch_mode();
        if mode != self.last_watch_mode {
            log::debug!(
                "watcher mode change: {:?} -> {:?}",
                self.last_watch_mode,
                mode
            );
            self.last_watch_mode = mode;
            self.watch_dirty = true;
            if let Some(interval) = mode.poll_interval() {
                self.next_poll_at = Instant::now() + interval;
            }
        }
        if let Some(interval) = mode.poll_interval() {
            if Instant::now() >= self.next_poll_at {
                log::debug!("watcher poll tick ({mode:?})");
                self.poll_prod_error_assets();
                self.next_poll_at = Instant::now() + interval;
            }
        }

        // 4. Apply any watch-set changes.
        self.reconcile_file_watcher();
    }

    /// One activity-aware poll tick: refresh prod folder existence for visible
    /// assets and re-validate any assets that currently have errors, so external
    /// prod changes are picked up while the window is unfocused.
    fn poll_prod_error_assets(&mut self) {
        self.rebuild_prod_folder_cache();
        let keys = self.error_keys_with_prod_folder();
        self.queue_revalidation(keys);
    }

    /// Schedule the next repaint needed to keep watcher debouncing, polling, and
    /// mode transitions alive — including while the window is unfocused. Returns
    /// the chosen delay, if any.
    fn watcher_repaint_after(&self) -> Option<Duration> {
        if self.file_watcher.is_none() {
            return None;
        }
        let now = Instant::now();
        let mut candidates: Vec<Duration> = Vec::new();
        for pending in self.watch_pending.values() {
            candidates.push(pending.deadline.saturating_duration_since(now));
        }
        let mode = self.watch_mode();
        if mode.poll_interval().is_some() {
            candidates.push(self.next_poll_at.saturating_duration_since(now));
        }
        if let Some(boundary) =
            file_watcher::WatchMode::boundary_after(self.last_activity_at.elapsed())
        {
            candidates.push(boundary);
        }
        candidates.into_iter().min()
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

    pub fn start_auth_login(&mut self) {
        if self.auth_rx.is_some() {
            return;
        }
        self.auth_login = None;
        let (tx, rx) = channel();
        thread::spawn(move || {
            let res = crate::auth::login_with_pkce(|login| {
                let _ = tx.send(AuthMsg::Started(login.clone()));
                if let Err(err) = open::that(&login.auth_url) {
                    log::warn!("Opening browser for Auth0 login failed: {err}");
                }
            });
            match res {
                Ok(tokens) => {
                    let _ = tx.send(AuthMsg::Success(tokens));
                }
                Err(err) => {
                    let _ = tx.send(AuthMsg::Error(format!("Login failed: {err}")));
                }
            }
        });
        self.auth_rx = Some(rx);
    }

    pub fn visible_validation_scope_snapshot(&self) -> VisibleValidationScope {
        let keys = self.visible_asset_keys();
        VisibleValidationScope {
            keys,
            selected_types: self.selected_types.clone(),
            selected_status_groups: self.selected_status_groups.clone(),
            author_filters: self.author_filters.clone(),
            search_query: self.search_query.clone(),
        }
    }

    fn visible_asset_keys(&self) -> Vec<RowKey> {
        let mut keys = Vec::new();
        for &asset_type in &self.selected_types {
            if let Some(AssetListState::Loaded(list)) = self.assets_by_type.get(&asset_type) {
                keys.extend(
                    list.assets
                        .iter()
                        .filter(|asset| {
                            table::asset_matches_filters(
                                asset,
                                &self.author_filters,
                                &self.selected_status_groups,
                            ) && slug_matches_search(&asset.slug, &self.search_query)
                        })
                        .map(|asset| RowKey {
                            asset_type,
                            slug: asset.slug.clone(),
                        }),
                );
            }
        }
        keys
    }

    pub fn start_validation_for_visible_assets(&mut self) {
        let scope = self.visible_validation_scope_snapshot();
        self.visible_validation_scope = scope.clone();
        self.start_validation_for_keys(scope.keys);
    }

    pub fn start_validation_if_visible_scope_changed(&mut self) {
        let scope = self.visible_validation_scope_snapshot();
        if scope == self.visible_validation_scope {
            return;
        }
        self.visible_validation_scope = scope.clone();
        self.rebuild_prod_folder_cache();
        self.rebuild_local_folder_cache();
        self.watch_dirty = true;
        self.start_validation_for_keys(scope.keys.clone());
        self.start_thumbnail_refresh_for_keys(scope.keys.clone());
        self.prune_thumbnail_revisions_to_visible_scope();
        for key in &scope.keys {
            for action in TransferAction::all() {
                self.start_transfer_estimate(key, action, false);
            }
        }
    }

    pub fn start_validation_for_keys(&mut self, keys: Vec<RowKey>) {
        let requests = self.validation_requests_for_keys(&keys);
        if requests.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        crate::validation::spawn(requests, tx);
        self.validation_job = Some(ValidationJob { rx });
    }

    pub fn dismiss_warning(&mut self, dismiss_key: String) {
        self.dismissed_warning_keys.insert(dismiss_key);
        if let Err(err) =
            crate::validation::save_dismissed_warning_keys(&self.dismissed_warning_keys)
        {
            self.error_banner = Some(format!("Failed to save dismissed warnings: {err}"));
        }
    }

    fn validation_requests_for_keys(&self, keys: &[RowKey]) -> Vec<crate::validation::Request> {
        let mut assets_by_key = HashMap::new();
        let mut status_options_by_type = HashMap::new();
        let mut seen_types = HashSet::new();
        for asset_type in keys.iter().map(|key| key.asset_type) {
            if !seen_types.insert(asset_type) {
                continue;
            }
            let Some(AssetListState::Loaded(list)) = self.assets_by_type.get(&asset_type) else {
                continue;
            };
            for asset in &list.assets {
                assets_by_key.insert((asset_type, asset.slug.as_str()), asset);
            }
            status_options_by_type.insert(asset_type, list.statuses.clone());
        }

        let mut requests = Vec::new();
        for key in keys {
            if self.jobs.contains_key(key) || self.plan_jobs.contains_key(key) {
                continue; // skip while a copy or plan is in progress
            }
            let Some(asset) = assets_by_key.get(&(key.asset_type, key.slug.as_str())) else {
                continue;
            };
            let status_options = status_options_by_type
                .get(&key.asset_type)
                .cloned()
                .unwrap_or_default();
            requests.push(crate::validation::Request {
                key: key.clone(),
                status: asset.status.clone(),
                status_options,
                local_root: self.local_root_for(key.asset_type).join(&key.slug),
                prod_root: self.prod_root_for(key.asset_type).join(&key.slug),
            });
        }
        requests
    }

    /// Drain asset API + job channels each frame.
    pub fn pump(&mut self, ctx: &egui::Context) {
        if let Some(res) = self
            .thumbnail_cleanup_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.thumbnail_cleanup_rx = None;
            if let Err(err) = res {
                log::warn!("Thumbnail cache cleanup failed: {err}");
            }
        }

        while let Some(msg) = self.auth_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match msg {
                AuthMsg::Started(login) => {
                    self.auth_login = Some(login);
                }
                AuthMsg::Success(tokens) => {
                    crate::auth::apply_tokens(&mut self.config, &tokens);
                    self.refresh_logged_in_identity();
                    if let Err(err) = crate::config::save(&self.config) {
                        self.error_banner = Some(format!("Failed to save login: {err}"));
                    }
                    self.auth_rx = None;
                    self.auth_login = None;
                    self.token_prompt_open = false;
                    // Drop any in-flight refreshes that used stale tokens. Their
                    // pending results (which would re-open the login prompt) are
                    // discarded by clearing the channels before notion_rx is drained
                    // later in this same pump() call.
                    self.notion_rx.clear();
                    self.refreshing.clear();
                    self.refresh_all_asset_types();
                    break;
                }
                AuthMsg::Error(message) => {
                    self.auth_rx = None;
                    self.error_banner = Some(message);
                    break;
                }
            }
        }

        if let Some((show_latest_notice_on_none, res)) =
            self.update_check.as_ref().and_then(|job| {
                job.rx
                    .try_recv()
                    .ok()
                    .map(|res| (job.show_latest_notice_on_none, res))
            })
        {
            self.update_check = None;
            match res {
                Ok(Some(info)) => {
                    self.update_dialog_open = info.minor_or_major_update;
                    self.pending_update = Some(info);
                }
                Ok(None) => {
                    if show_latest_notice_on_none {
                        self.set_version_notice("You already have the latest version");
                    }
                }
                Err(msg) => {
                    log::warn!("Update check failed: {msg}");
                }
            }
        }

        if let Some(res) = self
            .update_install
            .as_ref()
            .and_then(|job| job.rx.try_recv().ok())
        {
            self.update_install = None;
            if let Err(msg) = res {
                self.error_banner = Some(format!("Update failed: {msg}"));
            }
        }

        let estimate_keys: Vec<_> = self.transfer_estimate_jobs.keys().cloned().collect();
        for key in estimate_keys {
            let result = self
                .transfer_estimate_jobs
                .get(&key)
                .and_then(|job| job.rx.try_recv().ok());
            if let Some(result) = result {
                self.transfer_estimate_jobs.remove(&key);
                if let Ok(preview) = result {
                    self.transfer_estimates.insert(key, preview);
                }
            }
        }

        if let Some(dialog) = self.transfer_file_list_dialog.as_mut() {
            if let Some(result) = dialog.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                dialog.rx = None;
                dialog.loading = false;
                match result {
                    Ok(plan) => {
                        dialog.plan = Some(plan);
                        dialog.error = None;
                    }
                    Err(err) => {
                        dialog.plan = None;
                        dialog.error = Some(err);
                    }
                }
            }
        }

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
                    Ok((list, tokens)) => {
                        if let Some(tokens) = tokens {
                            crate::auth::apply_tokens(&mut self.config, &tokens);
                            self.refresh_logged_in_identity();
                            if let Err(err) = crate::config::save(&self.config) {
                                self.error_banner =
                                    Some(format!("Failed to save refreshed login: {err}"));
                            }
                        }
                        if cursor_guard {
                            // Buffer — apply once the cursor has been idle for 2s.
                            self.pending_notion.insert(t, list);
                        } else {
                            let _ = crate::cache::save(t.cache_name(), &list);
                            self.assets_by_type.insert(t, AssetListState::Loaded(list));
                            self.rebuild_prod_folder_cache();
                            self.rebuild_local_folder_cache();
                            self.start_validation_for_visible_assets();
                        }
                    }
                    Err(msg) => {
                        if crate::auth::is_auth_required_error(&msg) {
                            self.token_prompt_open = true;
                        }
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
            self.rebuild_local_folder_cache();
            self.start_validation_for_visible_assets();

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
                    Ok(tokens) => {
                        if let Some(tokens) = tokens {
                            crate::auth::apply_tokens(&mut self.config, &tokens);
                            self.refresh_logged_in_identity();
                            if let Err(err) = crate::config::save(&self.config) {
                                self.error_banner =
                                    Some(format!("Failed to save refreshed login: {err}"));
                            }
                        }
                        if let Some(AssetListState::Loaded(list)) =
                            self.assets_by_type.get(&key.asset_type)
                        {
                            let _ = crate::cache::save(key.asset_type.cache_name(), list);
                        }
                    }
                    Err(msg) => {
                        if crate::auth::is_auth_required_error(&msg) {
                            self.token_prompt_open = true;
                        }
                        jobs::set_asset_status(self, &key, job.previous);
                        self.start_validation_for_keys(vec![key.clone()]);
                        self.error_banner =
                            Some(format!("Status update failed for {}: {msg}", key.slug));
                    }
                }
            }
        }

        let title_rename_keys: Vec<RowKey> = self.title_renames.keys().cloned().collect();
        for key in title_rename_keys {
            let res_opt = self
                .title_renames
                .get(&key)
                .and_then(|job| job.rx.try_recv().ok());
            if let Some(res) = res_opt {
                let Some(job) = self.title_renames.remove(&key) else {
                    continue;
                };
                match res {
                    Ok(tokens) => {
                        if let Some(tokens) = tokens {
                            crate::auth::apply_tokens(&mut self.config, &tokens);
                            self.refresh_logged_in_identity();
                            if let Err(err) = crate::config::save(&self.config) {
                                self.error_banner =
                                    Some(format!("Failed to save refreshed login: {err}"));
                            }
                        }
                        // Update the slug in the asset list to match the renamed title.
                        if let Some(AssetListState::Loaded(list)) =
                            self.assets_by_type.get_mut(&key.asset_type)
                        {
                            if let Some(asset) = list.assets.iter_mut().find(|a| a.slug == key.slug)
                            {
                                asset.slug = job.new_title.clone();
                            }
                            let _ = crate::cache::save(key.asset_type.cache_name(), list);
                        }
                        // Re-validate with the new slug key.
                        let new_key = RowKey {
                            asset_type: key.asset_type,
                            slug: job.new_title,
                        };
                        self.start_validation_for_keys(vec![new_key]);
                    }
                    Err(msg) => {
                        if crate::auth::is_auth_required_error(&msg) {
                            self.token_prompt_open = true;
                        }
                        self.error_banner =
                            Some(format!("Title rename failed for {}: {msg}", key.slug));
                    }
                }
            }
        }

        let mut validation_finished = false;
        while let Some(msg) = self
            .validation_job
            .as_ref()
            .and_then(|job| job.rx.try_recv().ok())
        {
            match msg {
                crate::validation::Msg::RowValidated { key, findings } => {
                    self.validation_results.insert(key, findings);
                    // The error set may have changed; re-evaluate prod watches.
                    self.watch_dirty = true;
                }
                crate::validation::Msg::Finished => {
                    validation_finished = true;
                }
            }
        }
        if validation_finished {
            self.validation_job = None;
            // Start any re-validations that were coalesced while a job was running.
            if !self.pending_validation_keys.is_empty() {
                let keys: Vec<RowKey> = self.pending_validation_keys.drain().collect();
                self.start_validation_for_keys(keys);
            }
        }

        // Receive background prod/archive-folder cache rebuild result.
        if let Some((prod, archive)) = self
            .prod_cache_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.prod_folder_cache = prod;
            self.archive_folder_cache = archive;
            self.prod_cache_rx = None;
            self.watch_dirty = true;
        }

        let thumbnail_keys: Vec<RowKey> = self.thumbnail_jobs.keys().cloned().collect();
        for key in thumbnail_keys {
            let Some(job_revision) = self.thumbnail_jobs.get(&key).map(|job| job.revision) else {
                continue;
            };
            let revision_state = self.thumbnail_revisions.get(&key).cloned();
            let current_revision = revision_state
                .as_ref()
                .map(|revision| revision.load(Ordering::Acquire))
                .unwrap_or(job_revision);
            let result_opt = self
                .thumbnail_jobs
                .get(&key)
                .and_then(|job| job.rx.try_recv().ok());
            if let Some(result) = result_opt {
                self.thumbnail_jobs.remove(&key);
                match result {
                    Ok(result) => {
                        if result.revision != current_revision {
                            self.spawn_thumbnail_job_for_key(&key);
                            continue;
                        }
                        match thumbnails::load_thumbnail_texture(
                            ctx,
                            &result.cache_path,
                            &Self::thumbnail_texture_name(&key),
                        ) {
                            Ok(texture) => {
                                self.thumbnail_previews.insert(
                                    key,
                                    ThumbnailPreview {
                                        #[cfg(test)]
                                        signature: result.signature,
                                        texture,
                                    },
                                );
                            }
                            Err(err) => {
                                log::warn!(
                                    "Failed to load thumbnail texture for {}/{}: {err}",
                                    key.asset_type.folder(),
                                    key.slug
                                );
                            }
                        }
                    }
                    Err(err) => {
                        if err.contains("Missing thumbnail source") {
                            self.thumbnail_previews.remove(&key);
                            self.thumbnail_revisions.remove(&key);
                        } else if current_revision != job_revision {
                            self.spawn_thumbnail_job_for_key(&key);
                        } else {
                            log::warn!(
                                "Thumbnail generation failed for {}/{}: {err}",
                                key.asset_type.folder(),
                                key.slug
                            );
                        }
                    }
                }
            }
        }

        // Drain completed plan jobs and either open the conflict dialog or start copying.
        let plan_keys: Vec<RowKey> = self.plan_jobs.keys().cloned().collect();
        for k in plan_keys {
            let result_opt = self
                .plan_jobs
                .get(&k)
                .and_then(|job| job.rx.try_recv().ok());
            if let Some(result) = result_opt {
                let kind = self.plan_jobs.remove(&k).unwrap().kind;
                match result {
                    Err(e) => {
                        self.error_banner = Some(format!("Plan failed for {}: {e}", k.slug));
                    }
                    Ok(mut plan) => {
                        if kind.touches_archive() {
                            // Archive/Unarchive: the source (Prod / archive) is
                            // authoritative — auto-resolve conflicts to overwrite
                            // and never show the conflict dialog.
                            for f in plan.files.iter_mut() {
                                if matches!(f.action, Action::Conflict { .. }) {
                                    f.action = Action::Overwrite;
                                    plan.total_bytes_to_copy += f.size;
                                }
                            }
                            jobs::spawn_copy_job(self, k, kind, plan);
                        } else if !plan.conflicts().is_empty() {
                            self.pending_conflict = Some(PendingConflict { key: k, kind, plan });
                        } else {
                            jobs::spawn_copy_job(self, k, kind, plan);
                        }
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
                let finished_key = k.clone();
                if let Some(job) = self.jobs.remove(&k) {
                    if finished_successfully {
                        match job.kind {
                            TransferKind::Pull => {
                                self.update_prod_folder_cache_for(&k);
                                self.update_local_folder_cache_for(&k);
                            }
                            TransferKind::Push => {
                                self.update_prod_folder_cache_for(&k);
                                self.start_push_verification(k.clone(), job.plan.clone());
                            }
                            TransferKind::Unarchive => {
                                self.update_prod_folder_cache_for(&k);
                                self.update_archive_folder_cache_for(&k);
                            }
                            TransferKind::Archive => {
                                // Archive copy verified inline — now delete Prod.
                                self.update_archive_folder_cache_for(&k);
                                self.start_archive_delete(k.clone(), job.started_at, &job.plan);
                            }
                        }
                        // Archive toasts after its delete step completes.
                        if !matches!(job.kind, TransferKind::Archive) {
                            self.row_toasts.insert(
                                k.clone(),
                                RowToast {
                                    text: format!(
                                        "{} in {}",
                                        job.kind.done_label(),
                                        fmt_duration(job.started_at.elapsed())
                                    ),
                                    created_at: Instant::now(),
                                },
                            );
                        }
                    } else if job.kind.touches_archive() {
                        // Cancelled/failed archive or unarchive: a partial
                        // archive/Prod folder may exist — refresh both caches.
                        self.update_prod_folder_cache_for(&k);
                        self.update_archive_folder_cache_for(&k);
                    }
                }
                // Re-run validation now that the copy is no longer in progress.
                self.start_validation_for_keys(vec![finished_key.clone()]);
                // Clear and restart estimates for this key (file state has changed).
                self.clear_transfer_estimates_for_key(&finished_key);
                for action in TransferAction::all() {
                    self.start_transfer_estimate(&finished_key, action, true);
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
        self.pump_archive_deletes();
        self.row_toasts
            .retain(|_, toast| toast.created_at.elapsed() < Duration::from_secs(5));

        self.pump_file_watcher();
    }

    pub fn local_root_for(&self, t: AssetType) -> PathBuf {
        self.config.local_root.join(t.folder())
    }
    pub fn prod_root_for(&self, t: AssetType) -> PathBuf {
        self.config.prod_root.join(t.folder())
    }
    /// Root for an asset type's archived files, e.g. `A:\Textures`.
    pub fn archive_root_for(&self, t: AssetType) -> PathBuf {
        self.config.archive_root.join(t.folder())
    }

    /// Re-check whether the archive folder exists for a single asset.
    pub fn update_archive_folder_cache_for(&mut self, key: &RowKey) {
        let exists = self.archive_root_for(key.asset_type).join(&key.slug).is_dir();
        self.archive_folder_cache.insert(key.clone(), exists);
    }

    /// True when the asset's loaded status is in the Complete group ("Done").
    pub fn asset_is_complete(&self, key: &RowKey) -> bool {
        matches!(
            self.assets_by_type.get(&key.asset_type),
            Some(AssetListState::Loaded(list))
                if list.assets.iter().any(|a| a.slug == key.slug
                    && a.status
                        .as_ref()
                        .map(|s| s.group == crate::notion::StatusGroup::Complete)
                        .unwrap_or(false))
        )
    }

    /// Request archiving an asset: opens the confirmation prompt. The actual
    /// copy/verify/delete only starts once the user confirms.
    pub fn request_archive(&mut self, key: &RowKey) {
        if self.plan_jobs.contains_key(key)
            || self.jobs.contains_key(key)
            || self.archive_deletes.contains_key(key)
        {
            return;
        }
        self.pending_archive = Some(key.clone());
    }

    pub fn open_settings(&mut self) {
        self.settings_local_root_input = self.config.local_root.display().to_string();
        self.settings_affinity_path_input = self.config.affinity_path.display().to_string();
        self.settings_open_notion_links_in_desktop_app =
            self.config.open_notion_links_in_desktop_app;
        self.settings_open = true;
    }

    pub fn refresh_logged_in_identity(&mut self) {
        self.logged_in_identity = if self.config.has_access_token() {
            match crate::auth::fetch_logged_in_identity(&self.config.auth_access_token) {
                Ok(identity) => Some(identity),
                Err(err) => {
                    log::warn!("Failed to fetch Auth0 userinfo: {err}");
                    crate::auth::logged_in_identity(&self.config.auth_access_token)
                }
            }
        } else {
            None
        };
    }

    pub fn is_admin(&self) -> bool {
        self.logged_in_identity
            .as_ref()
            .map(|identity| {
                identity
                    .role
                    .split([',', ';', '|'])
                    .any(|role| role.trim().eq_ignore_ascii_case("admin"))
            })
            .unwrap_or(false)
    }

    fn start_push_verification(&mut self, key: RowKey, plan: crate::copy::plan::Plan) {
        let (tx, rx) = std::sync::mpsc::channel();
        let progress = Arc::new(JobProgress::default());
        crate::copy::job::spawn_verification(plan.files, progress.clone(), tx);
        self.verifications.insert(
            key,
            VerificationJob {
                kind: TransferKind::Push,
                progress,
                rx,
            },
        );
    }

    /// After an Archive copy fully succeeds (verified inline), delete the asset's
    /// Prod folder on a background thread, guarded by `delete_prod_after_archive`.
    fn start_archive_delete(&mut self, key: RowKey, started_at: Instant, plan: &Plan) {
        let src_root = self.prod_root_for(key.asset_type).join(&key.slug);
        // Every file accounted for in the archive: copied+verified this run, or
        // already present and unchanged (skipped during copy). The delete thread
        // refuses to delete Prod if it has gained any *other* (un-archived) file.
        let verified: std::collections::HashSet<PathBuf> =
            plan.files.iter().map(|f| f.rel_path.clone()).collect();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(delete_prod_after_archive(&src_root, &verified));
        });
        self.archive_deletes
            .insert(key, ArchiveDelete { started_at, rx });
    }

    /// Drain completed post-archive Prod deletions.
    fn pump_archive_deletes(&mut self) {
        let keys: Vec<RowKey> = self.archive_deletes.keys().cloned().collect();
        for k in keys {
            let Some(result) = self
                .archive_deletes
                .get(&k)
                .and_then(|d| d.rx.try_recv().ok())
            else {
                continue;
            };
            let started_at = self.archive_deletes.remove(&k).map(|d| d.started_at);
            self.update_prod_folder_cache_for(&k);
            self.update_archive_folder_cache_for(&k);
            match result {
                Ok(()) => {
                    self.row_toasts.insert(
                        k.clone(),
                        RowToast {
                            text: format!(
                                "Archived in {}",
                                fmt_duration(started_at.map(|s| s.elapsed()).unwrap_or_default())
                            ),
                            created_at: Instant::now(),
                        },
                    );
                }
                Err(e) => {
                    self.error_banner = Some(format!(
                        "Archived {} but could not delete all Prod files: {e}",
                        k.slug
                    ));
                }
            }
            self.start_validation_for_keys(vec![k.clone()]);
            self.start_thumbnail_refresh_for_keys(vec![k]);
        }
    }
}

fn fmt_duration(duration: Duration) -> String {
    let secs = duration.as_secs().max(1);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Delete an asset's Prod slug folder after its archive copy has been verified.
///
/// As a final safety gate against concurrent modification, this re-walks Prod
/// and refuses to delete if it contains any file that was neither archived (and
/// thus present in `verified`) nor an intentionally-discarded `work/*.tif`. Such
/// a file would have appeared after the archive plan was built and is therefore
/// not in the verified archive — deleting it would be silent data loss.
fn delete_prod_after_archive(
    src_root: &std::path::Path,
    verified: &std::collections::HashSet<PathBuf>,
) -> Result<(), String> {
    for entry in walkdir::WalkDir::new(src_root).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.ends_with(".partial") {
            continue;
        }
        let abs = entry.path();
        let Ok(rel) = abs.strip_prefix(src_root) else {
            continue;
        };
        // Skipped during archiving (discarded) — safe to delete.
        if crate::copy::plan::is_work_tif(rel, &file_name) {
            continue;
        }
        // Archived and verified — safe to delete.
        if verified.contains(rel) {
            continue;
        }
        return Err(format!(
            "Prod gained an un-archived file since archiving started ({}); Prod was NOT deleted",
            rel.display()
        ));
    }
    match std::fs::remove_dir_all(src_root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn format_active_file_action(
    key: &RowKey,
    kind: TransferKind,
    current_file: Option<&str>,
) -> String {
    let target = match current_file {
        Some(file) if !file.is_empty() => abbreviate_status_path(file),
        _ => format!("{}/{}", key.asset_type.folder(), key.slug),
    };
    match kind {
        TransferKind::Push => format!("Uploading {target} to Prod"),
        TransferKind::Pull => format!("Downloading {target} from Prod"),
        TransferKind::Archive => format!("Archiving {target}"),
        TransferKind::Unarchive => format!("Unarchiving {target}"),
    }
}

fn abbreviate_status_path(path: &str) -> String {
    let parts = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 3 {
        return parts.join("\\");
    }
    format!("...{}", parts[parts.len() - 3..].join("\\"))
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
            format_active_file_action(key, job.kind, current_file.as_deref())
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
                Some(file) if !file.is_empty() => abbreviate_status_path(file),
                _ => format!("{}/{}", key.asset_type.folder(), key.slug),
            };
            format!("Verifying {target}")
        })
        .or_else(|| {
            state
                .archive_deletes
                .keys()
                .min_by(|a, b| {
                    a.asset_type
                        .order()
                        .cmp(&b.asset_type.order())
                        .then_with(|| a.slug.cmp(&b.slug))
                })
                .map(|key| {
                    format!("Removing Prod files {}/{}", key.asset_type.folder(), key.slug)
                })
        })
}

fn logged_in_status_text(state: &AppState) -> Option<String> {
    state
        .logged_in_identity
        .as_ref()
        .map(|identity| format!("Logged in as {} [{}]", identity.name, identity.role))
        .or_else(|| {
            state
                .config
                .has_access_token()
                .then(|| "Logged in as Unknown [Unknown]".to_string())
        })
}

fn draw_status_bar(state: &mut AppState, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(
                layout::STATUS_BAR_MARGIN_X,
                layout::STATUS_BAR_MARGIN_Y,
            ))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    draw_status_bar_primary(state, ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        draw_version_status(state, ui);
                    });
                });
            });
    });
}

pub(super) fn slug_matches_search(slug: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let slug_lower = slug.to_lowercase();
    query
        .split_whitespace()
        .all(|word| slug_lower.contains(&word.to_lowercase()))
}

/// If `path` lies under `root`, return the first path component below `root`
/// (the asset slug). Comparison is case-insensitive to tolerate Windows path
/// casing differences between the configured root and watcher event paths.
fn slug_under_root(root: &std::path::Path, path: &std::path::Path) -> Option<String> {
    let mut root_components = root.components();
    let mut path_components = path.components();
    loop {
        match root_components.next() {
            Some(root_part) => {
                let path_part = path_components.next()?;
                if !components_eq_ci(root_part, path_part) {
                    return None;
                }
            }
            None => break,
        }
    }
    let slug = path_components.next()?;
    let slug = slug.as_os_str().to_string_lossy().to_string();
    if slug.is_empty() {
        None
    } else {
        Some(slug)
    }
}

fn components_eq_ci(a: std::path::Component, b: std::path::Component) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

pub(super) fn latest_update_tag_for_install<F>(lookup: F) -> Result<String, String>
where
    F: FnOnce() -> Result<Option<crate::updater::UpdateInfo>, String>,
{
    let Some(update) = lookup()? else {
        return Err("No newer update found".into());
    };
    Ok(update.tag)
}

fn draw_status_bar_primary(state: &mut AppState, ui: &mut egui::Ui) {
    if let Some(err) = state.error_banner.clone() {
        ui.horizontal(|ui| {
            ui.colored_label(colors::ERROR_BANNER, err);
            let tex = x_icon_texture(ui.ctx());
            let resp = ui.add(
                egui::Image::new(egui::load::SizedTexture::new(
                    tex.id(),
                    egui::vec2(layout::STATUS_BAR_ICON_SIZE, layout::STATUS_BAR_ICON_SIZE),
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
        return;
    }

    if let Some(status) = active_file_action_status(state) {
        ui.label(egui::RichText::new(status).color(colors::TEXT_DISABLED));
        return;
    }

    let refreshing = !state.refreshing.is_empty()
        && state
            .selected_types
            .iter()
            .any(|t| state.refreshing.contains(t));
    if refreshing {
        ui.label(egui::RichText::new("Updating Notion data...").color(colors::TEXT_DISABLED));
        return;
    }

    if let Some(status) = logged_in_status_text(state) {
        ui.label(egui::RichText::new(status).color(colors::TEXT_DISABLED));
    }
}

fn draw_version_status(state: &mut AppState, ui: &mut egui::Ui) {
    state.clear_expired_version_notice(Instant::now());
    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    if state.update_install.is_some() {
        ui.label(egui::RichText::new("Installing update...").color(colors::TEXT_DISABLED));
        return;
    }
    let label = if let Some(notice) = state.version_notice.as_ref() {
        notice.message.clone()
    } else if state.pending_update.is_some() {
        format!("{current} - New version available")
    } else {
        current
    };
    let response = ui
        .add(
            egui::Label::new(egui::RichText::new(label).color(colors::TEXT_DISABLED))
                .sense(egui::Sense::click()),
        )
        .on_hover_text("Check for updates")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        state.start_update_check_force();
    }
}

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let gained_focus = ctx.input(|i| state.focus_refresh.update(i.focused, Instant::now()));

    // Track PHASE-window activity for the activity-aware prod watch schedule:
    // any mouse movement, keyboard/text input, or focus change counts as activity.
    let (focused_now, had_input) = ctx.input(|i| {
        let input = i.pointer.delta() != egui::Vec2::ZERO || !i.events.is_empty();
        (i.focused, input)
    });
    if had_input || focused_now != state.watcher_was_focused {
        state.last_activity_at = Instant::now();
    }
    state.watcher_was_focused = focused_now;

    // Skip the focus-triggered refresh if login is in progress — a fresh refresh
    // will be started by AuthMsg::Success once authentication completes.
    if gained_focus && !state.token_prompt_open && state.auth_rx.is_none() {
        state.refresh_all_asset_types();
        state.rebuild_prod_folder_cache();
        state.rebuild_local_folder_cache();
        state.start_validation_for_visible_assets();
        // Restart estimates with force=true so new jobs run, but old values remain
        // visible until results arrive (avoids flickering preview text on focus gain).
        state.transfer_estimate_jobs.clear();
        state.start_transfer_estimates_for_visible(true);
    }

    egui::TopBottomPanel::top("menu").show(ctx, |ui| {
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(
                0.0,
                layout::TOP_BAR_VERTICAL_PADDING,
            ))
            .show(ui, |ui| menu::draw(state, ui));
    });
    state.start_validation_if_visible_scope_changed();
    scripts::pump(state);
    dialogs::token_prompt(state, ctx);
    dialogs::settings(state, ctx);
    dialogs::draw(state, ctx);
    draw_update_prompt(state, ctx);
    draw_create_prod_folder_prompt(state, ctx);
    draw_delete_local_folder_prompt(state, ctx);
    draw_archive_prompt(state, ctx);
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
        // Keep ticking while a browser login is in flight, and while the asset
        // fetch it triggers is running, so AuthMsg::Success is drained and the
        // post-login Notion refresh lands even if the window never lost and
        // regained focus (e.g. a cached Auth0 session redirects instantly).
        || state.auth_rx.is_some()
        || !state.notion_rx.is_empty()
        || !state.row_toasts.is_empty()
        || !state.jobs.is_empty()
        || !state.plan_jobs.is_empty()
        || state.prod_cache_rx.is_some()
        || !state.verifications.is_empty()
        || !state.archive_deletes.is_empty()
        || state.validation_job.is_some()
        || state.update_check.is_some()
        || state.update_install.is_some()
        || !state.transfer_estimate_jobs.is_empty()
        || !state.thumbnail_jobs.is_empty()
        || state.thumbnail_cleanup_rx.is_some()
        || !state.script_jobs.is_empty()
        || !state.script_queue.is_empty()
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    // Keep the loop alive for watcher debouncing, polling, and mode transitions
    // (including while the window is unfocused).
    if let Some(delay) = state.watcher_repaint_after() {
        ctx.request_repaint_after(delay);
    }
}

fn draw_update_prompt(state: &mut AppState, ctx: &egui::Context) {
    if !state.update_dialog_open {
        return;
    }
    let Some(update) = state.pending_update.clone() else {
        state.update_dialog_open = false;
        return;
    };

    let mut install = false;
    let mut close = false;
    let mut notes = update.notes.clone();
    egui::Window::new(format!("PHASE {} available", update.tag))
        .collapsible(false)
        .resizable(true)
        .default_width(layout::UPDATE_DIALOG_WIDTH)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!("A new PHASE version is available: {}", update.tag));
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            ui.label("Release notes:");
            egui::ScrollArea::vertical()
                .max_height(layout::UPDATE_DIALOG_SCROLL_HEIGHT)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut notes)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .interactive(false),
                    );
                });
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            ui.horizontal(|ui| {
                if ui.button("Update").clicked() {
                    install = true;
                }
                if ui.button("Not now").clicked() {
                    close = true;
                }
            });
        });

    if install {
        state.update_dialog_open = false;
        state.start_update_install();
    } else if close {
        state.update_dialog_open = false;
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
        .default_width(layout::VERIFICATION_DIALOG_WIDTH)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("The push finished, but post-copy verification found a mismatch.");
            ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);
            ui.label("Problem file:");
            ui.monospace(&rel_path);
            ui.add_space(layout::TOP_BAR_EDGE_PADDING);
            ui.label("Verification error:");
            ui.monospace(&error);
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
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
        start_job(state, &key, TransferAction::default_push());
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

fn delete_local_folder(state: &mut AppState, key: &RowKey) {
    let local_folder = state.local_root_for(key.asset_type).join(&key.slug);
    match std::fs::remove_dir_all(&local_folder) {
        Ok(()) => {
            state.row_toasts.insert(
                key.clone(),
                RowToast {
                    text: "Deleted local files".into(),
                    created_at: Instant::now(),
                },
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            state.error_banner = Some(format!(
                "Could not delete local files for {}: {err}",
                key.slug
            ));
            return;
        }
    }
    state.rebuild_local_folder_cache();
    state.start_validation_for_visible_assets();
}

fn draw_archive_prompt(state: &mut AppState, ctx: &egui::Context) {
    let Some(key) = state.pending_archive.clone() else {
        return;
    };
    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new("Archive asset?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!(
                "Copy the Prod files for {} to the archive drive, then delete them from Prod?",
                key.slug
            ));
            ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);
            ui.colored_label(
                colors::MSG_WARNING,
                ".tif files in the work folder will be discarded, not archived.",
            );
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            ui.horizontal(|ui| {
                if ui.button("Archive").clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if confirm {
        state.pending_archive = None;
        start_archive(state, &key);
    } else if cancel {
        state.pending_archive = None;
    }
}

fn draw_delete_local_folder_prompt(state: &mut AppState, ctx: &egui::Context) {
    let Some(key) = state.pending_local_folder_delete.clone() else {
        return;
    };
    egui::Window::new("Delete local files?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!("Delete the local folder for {}?", key.slug));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    delete_local_folder(state, &key);
                    state.pending_local_folder_delete = None;
                }
                if ui.button("Cancel").clicked() {
                    state.pending_local_folder_delete = None;
                }
            });
        });
}

#[cfg(test)]
mod tests;
