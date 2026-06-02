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
use crate::copy::plan::{build_plan_with_pull_filter, Action, Direction, Plan, PullFilterMode};
use crate::auth::{AuthTokens, BrowserLogin};
use crate::notion::{AssetList, AssetStatus, StatusOption};

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
    pub direction: Direction,
    pub rx: Receiver<Result<Plan, String>>,
}

pub struct TransferEstimateJob {
    pub rx: Receiver<Result<u64, String>>,
}

pub enum AuthMsg {
    Started(BrowserLogin),
    Success(AuthTokens),
    Error(String),
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
    pub status_updates: HashMap<RowKey, StatusUpdateJob>,
    pub title_renames: HashMap<RowKey, TitleRenameJob>,
    pub notion_rx: HashMap<AssetType, Receiver<Result<(AssetList, Option<AuthTokens>), String>>>,
    pub pending_conflict: Option<PendingConflict>,
    pub pending_verification_failure: Option<PendingVerificationFailure>,
    pub pending_prod_folder_create: Option<RowKey>,
    pub pending_local_folder_delete: Option<RowKey>,
    pub row_toasts: HashMap<RowKey, RowToast>,
    pub published_assets: crate::polyhaven::PublishedAssets,
    pub published_rx: Option<Receiver<Result<crate::polyhaven::PublishedAssets, String>>>,
    pub refreshing_published: bool,
    pub token_prompt_open: bool,
    pub token_input: String,
    pub auth_login: Option<BrowserLogin>,
    pub auth_rx: Option<Receiver<AuthMsg>>,
    pub settings_open: bool,
    pub settings_local_root_input: String,
    pub settings_skip_pull_raw_tif_if_many_work_tifs: bool,
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
    /// Receiver for an in-flight background rebuild of `prod_folder_cache`.
    pub prod_cache_rx: Option<Receiver<HashMap<RowKey, bool>>>,
    /// Cached result of `is_dir()` for each asset's local working folder.
    /// Rebuilt synchronously (local disk) on focus gain and after pulls.
    pub local_folder_cache: HashMap<RowKey, bool>,
    pub dismissed_warning_keys: HashSet<String>,
    pub validation_results: HashMap<RowKey, Vec<crate::validation::Finding>>,
    pub validation_job: Option<ValidationJob>,
    pub visible_validation_scope: VisibleValidationScope,
    pub update_check_rx: Option<Receiver<Result<Option<crate::updater::UpdateInfo>, String>>>,
    pub pending_update: Option<crate::updater::UpdateInfo>,
    pub update_dialog_open: bool,
    pub update_install: Option<UpdateInstallJob>,
    pub transfer_estimates: HashMap<(RowKey, Direction), u64>,
    pub transfer_estimate_jobs: HashMap<(RowKey, Direction), TransferEstimateJob>,
    pub search_query: String,
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

fn current_update_check_day() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0)
}

fn should_check_for_update(last_check_day: Option<u64>, current_day: u64) -> bool {
    last_check_day != Some(current_day)
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
            status_updates: HashMap::new(),
            title_renames: HashMap::new(),
            notion_rx: HashMap::new(),
            pending_conflict: None,
            pending_verification_failure: None,
            pending_prod_folder_create: None,
            pending_local_folder_delete: None,
            row_toasts: HashMap::new(),
            published_assets: crate::cache::load(crate::polyhaven::cache_name())
                .unwrap_or_default(),
            published_rx: None,
            refreshing_published: false,
            token_prompt_open: false,
            token_input: String::new(),
            auth_login: None,
            auth_rx: None,
            settings_open: false,
            settings_local_root_input: String::new(),
            settings_skip_pull_raw_tif_if_many_work_tifs: false,
            settings_open_notion_links_in_desktop_app: false,
            refreshing: HashSet::new(),
            pending_notion: HashMap::new(),
            cursor_moved_in_table_at: None,
            focus_refresh: focus_refresh::State::default(),
            prod_folder_cache: HashMap::new(),
            prod_cache_rx: None,
            local_folder_cache: HashMap::new(),
            dismissed_warning_keys: crate::validation::load_dismissed_warning_keys()
                .unwrap_or_default(),
            validation_results: HashMap::new(),
            validation_job: None,
            visible_validation_scope: VisibleValidationScope::default(),
            update_check_rx: None,
            pending_update: None,
            update_dialog_open: false,
            update_install: None,
            transfer_estimates: HashMap::new(),
            transfer_estimate_jobs: HashMap::new(),
            search_query: String::new(),
        };
        s.token_prompt_open = !s.config.has_access_token() && !s.config.can_refresh_access_token();
        s.token_input.clear();
        s.settings_local_root_input = s.config.local_root.display().to_string();
        s.settings_skip_pull_raw_tif_if_many_work_tifs =
            s.config.skip_pull_raw_tif_if_many_work_tifs;
        s.settings_open_notion_links_in_desktop_app =
            s.config.open_notion_links_in_desktop_app;
        // Warm the UI from cache immediately, then refresh in the background.
        for t in [AssetType::Hdris, AssetType::Textures] {
            if let Some(cached) = crate::cache::load(t.cache_name()) {
                s.assets_by_type.insert(t, AssetListState::Loaded(cached));
            }
        }
        s.rebuild_prod_folder_cache();
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
        let available =
            authors::filter_options(list.assets.iter().map(|asset| asset.author.as_str()));
        self.author_filters
            .iter()
            .filter(|filter| available.iter().any(|author| author == *filter))
            .cloned()
            .collect()
    }

    fn start_update_check(&mut self) {
        if self.update_check_rx.is_some() {
            return;
        }
        let today = current_update_check_day();
        if !should_check_for_update(self.config.last_update_check_day, today) {
            return;
        }
        self.config.last_update_check_day = Some(today);
        let _ = crate::config::save(&self.config);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let res = crate::updater::check_for_update().map_err(|err| err.to_string());
            let _ = tx.send(res);
        });
        self.update_check_rx = Some(rx);
    }

    pub fn start_update_install(&mut self) {
        if self.update_install.is_some() {
            return;
        }
        let Some(update) = self.pending_update.clone() else {
            return;
        };
        let (tx, rx) = channel();
        thread::spawn(move || {
            let res = crate::updater::install_update_and_restart(&update.tag)
                .map_err(|err| err.to_string());
            let _ = tx.send(res);
        });
        self.update_install = Some(UpdateInstallJob { rx });
    }

    pub fn start_transfer_estimate(&mut self, key: &RowKey, direction: Direction) {
        let estimate_key = (key.clone(), direction);
        if self.transfer_estimates.contains_key(&estimate_key)
            || self.transfer_estimate_jobs.contains_key(&estimate_key)
            || self.plan_jobs.contains_key(key)
            || self.jobs.contains_key(key)
        {
            return;
        }
        let src_root = match direction {
            Direction::Pull => self.prod_root_for(key.asset_type).join(&key.slug),
            Direction::Push => self.local_root_for(key.asset_type).join(&key.slug),
        };
        let dst_root = match direction {
            Direction::Pull => self.local_root_for(key.asset_type).join(&key.slug),
            Direction::Push => self.prod_root_for(key.asset_type).join(&key.slug),
        };
        let pull_filter = match direction {
            Direction::Pull if self.config.skip_pull_raw_tif_if_many_work_tifs => {
                PullFilterMode::SkipRawAndTifWhenWorkTifsExceed { threshold: 30 }
            }
            Direction::Pull | Direction::Push => PullFilterMode::None,
        };
        let (tx, rx) = channel();
        thread::spawn(move || {
            let result = build_plan_with_pull_filter(direction, &src_root, &dst_root, pull_filter)
                .map(|plan| plan.total_bytes_to_copy)
                .map_err(|err| err.to_string());
            let _ = tx.send(result);
        });
        self.transfer_estimate_jobs
            .insert(estimate_key, TransferEstimateJob { rx });
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

    /// Kick off a background rebuild of the prod-folder existence cache.
    /// When the thread finishes, `pump()` will receive the result and swap it in.
    pub fn rebuild_prod_folder_cache(&mut self) {
        let to_check: Vec<(RowKey, std::path::PathBuf)> = self
            .visible_asset_keys()
            .into_iter()
            .map(|key| {
                let path = self.prod_root_for(key.asset_type).join(&key.slug);
                (key, path)
            })
            .collect();
        if to_check.is_empty() {
            self.prod_folder_cache.clear();
            return;
        }
        let (tx, rx) = channel();
        thread::spawn(move || {
            let cache: HashMap<RowKey, bool> =
                to_check.into_iter().map(|(k, p)| (k, p.is_dir())).collect();
            let _ = tx.send(cache);
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

    /// Update the local cache for a single asset after a pull finishes.
    pub fn update_local_folder_cache_for(&mut self, key: &RowKey) {
        let exists = self.local_root_for(key.asset_type).join(&key.slug).is_dir();
        self.local_folder_cache.insert(key.clone(), exists);
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
        self.start_validation_for_keys(scope.keys);
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
    pub fn pump(&mut self) {
        while let Some(msg) = self.auth_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match msg {
                AuthMsg::Started(login) => {
                    self.auth_login = Some(login);
                }
                AuthMsg::Success(tokens) => {
                    crate::auth::apply_tokens(&mut self.config, &tokens);
                    if let Err(err) = crate::config::save(&self.config) {
                        self.error_banner = Some(format!("Failed to save login: {err}"));
                    }
                    self.auth_rx = None;
                    self.auth_login = None;
                    self.token_prompt_open = false;
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

        if let Some(res) = self
            .update_check_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.update_check_rx = None;
            match res {
                Ok(Some(info)) => {
                    self.update_dialog_open = info.minor_or_major_update;
                    self.pending_update = Some(info);
                }
                Ok(None) => {}
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
                if let Ok(bytes) = result {
                    self.transfer_estimates.insert(key, bytes);
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
                        set_asset_status(self, &key, job.previous);
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
                            if let Err(err) = crate::config::save(&self.config) {
                                self.error_banner =
                                    Some(format!("Failed to save refreshed login: {err}"));
                            }
                        }
                        // Update the slug in the asset list to match the renamed title.
                        if let Some(AssetListState::Loaded(list)) =
                            self.assets_by_type.get_mut(&key.asset_type)
                        {
                            if let Some(asset) =
                                list.assets.iter_mut().find(|a| a.slug == key.slug)
                            {
                                asset.slug = job.new_title.clone();
                            }
                            let _ = crate::cache::save(key.asset_type.cache_name(), list);
                        }
                        // Re-validate with the new slug key.
                        let new_key = RowKey { asset_type: key.asset_type, slug: job.new_title };
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
                }
                crate::validation::Msg::Finished => {
                    validation_finished = true;
                }
            }
        }
        if validation_finished {
            self.validation_job = None;
        }

        // Receive background prod-folder cache rebuild result.
        if let Some(cache) = self
            .prod_cache_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.prod_folder_cache = cache;
            self.prod_cache_rx = None;
        }

        // Drain completed plan jobs and either open the conflict dialog or start copying.
        let plan_keys: Vec<RowKey> = self.plan_jobs.keys().cloned().collect();
        for k in plan_keys {
            let result_opt = self
                .plan_jobs
                .get(&k)
                .and_then(|job| job.rx.try_recv().ok());
            if let Some(result) = result_opt {
                let direction = self.plan_jobs.remove(&k).unwrap().direction;
                match result {
                    Err(e) => {
                        self.error_banner = Some(format!("Plan failed for {}: {e}", k.slug));
                    }
                    Ok(plan) => {
                        if !plan.conflicts().is_empty() {
                            self.pending_conflict = Some(PendingConflict {
                                key: k,
                                direction,
                                plan,
                            });
                        } else {
                            spawn_copy_job(self, k, direction, plan);
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
                        self.update_prod_folder_cache_for(&k);
                        if job.direction == Direction::Pull {
                            self.update_local_folder_cache_for(&k);
                        }
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
                // Re-run validation now that the copy is no longer in progress.
                self.start_validation_for_keys(vec![finished_key]);
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

    pub fn open_settings(&mut self) {
        self.settings_local_root_input = self.config.local_root.display().to_string();
        self.settings_skip_pull_raw_tif_if_many_work_tifs =
            self.config.skip_pull_raw_tif_if_many_work_tifs;
        self.settings_open_notion_links_in_desktop_app =
            self.config.open_notion_links_in_desktop_app;
        self.settings_open = true;
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
    // Ignore if already planning or copying.
    if state.plan_jobs.contains_key(key) || state.jobs.contains_key(key) {
        return;
    }
    // Clear stale validation messages and any previous toast so the row is clean
    // while the job runs. Fresh validation fires automatically after the job finishes.
    state.validation_results.remove(key);
    state.row_toasts.remove(key);
    state.transfer_estimates.remove(&(key.clone(), Direction::Pull));
    state.transfer_estimates.remove(&(key.clone(), Direction::Push));
    let src_root = match direction {
        Direction::Pull => state.prod_root_for(key.asset_type).join(&key.slug),
        Direction::Push => state.local_root_for(key.asset_type).join(&key.slug),
    };
    let dst_root = match direction {
        Direction::Pull => state.local_root_for(key.asset_type).join(&key.slug),
        Direction::Push => state.prod_root_for(key.asset_type).join(&key.slug),
    };
    let pull_filter = match direction {
        Direction::Pull if state.config.skip_pull_raw_tif_if_many_work_tifs => {
            PullFilterMode::SkipRawAndTifWhenWorkTifsExceed { threshold: 30 }
        }
        Direction::Pull => PullFilterMode::None,
        Direction::Push => PullFilterMode::None,
    };

    let (tx, rx) = channel::<Result<Plan, String>>();
    thread::spawn(move || {
        let result =
            build_plan_with_pull_filter(direction, &src_root, &dst_root, pull_filter)
                .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    state.plan_jobs.insert(key.clone(), PlanJob { direction, rx });
}

fn spawn_copy_job(state: &mut AppState, key: RowKey, direction: Direction, plan: Plan) {
    let (tx, rx) = channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan.clone(), progress.clone(), tx);
    state.jobs.insert(
        key,
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
    state.start_validation_for_keys(vec![key.clone()]);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut config = state.config.clone();
    let page_id = page_id.to_string();
    let requested_for_thread = requested.clone();
    thread::spawn(move || {
        let res = (|| {
            let before = (
                config.auth_access_token.clone(),
                config.auth_refresh_token.clone(),
                config.auth_expires_at,
            );
            let token = crate::auth::ensure_access_token(&mut config)?;
            crate::notion::update_page_status(&token, &page_id, &requested_for_thread)?;
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
            Ok(tokens)
        })()
        .map_err(|e: anyhow::Error| e.to_string());
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

pub fn start_title_rename(state: &mut AppState, key: &RowKey, page_id: &str, new_title: &str) {
    if state.title_renames.contains_key(key) {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let mut config = state.config.clone();
    let page_id = page_id.to_string();
    let new_title_str = new_title.to_string();
    let new_title_for_job = new_title_str.clone();
    thread::spawn(move || {
        let res = (|| {
            let before = (
                config.auth_access_token.clone(),
                config.auth_refresh_token.clone(),
                config.auth_expires_at,
            );
            let token = crate::auth::ensure_access_token(&mut config)?;
            crate::notion::rename_page_title(&token, &page_id, &new_title_str)?;
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
            Ok(tokens)
        })()
        .map_err(|e: anyhow::Error| e.to_string());
        let _ = tx.send(res);
    });
    state.title_renames.insert(
        key.clone(),
        TitleRenameJob {
            rx,
            new_title: new_title_for_job,
        },
    );
}

fn status_from_option(option: &StatusOption) -> AssetStatus {
    AssetStatus {
        id: option.id.clone(),
        name: option.name.clone(),
        color: option.color.clone(),
        group: option.group,
        sort_order: option.sort_order,
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
    spawn_copy_job(state, pc.key, pc.direction, plan);
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
        Some(file) if !file.is_empty() => abbreviate_status_path(file),
        _ => format!("{}/{}", key.asset_type.folder(), key.slug),
    };
    let suffix = match direction {
        Direction::Pull => "from Prod",
        Direction::Push => "to Prod",
    };
    format!("{verb} {target} {suffix}")
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
                Some(file) if !file.is_empty() => abbreviate_status_path(file),
                _ => format!("{}/{}", key.asset_type.folder(), key.slug),
            };
            format!("Verifying {target}")
        })
}

fn draw_status_bar(state: &mut AppState, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(4.0, 2.0))
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

fn draw_status_bar_primary(state: &mut AppState, ui: &mut egui::Ui) {
    if let Some(err) = state.error_banner.clone() {
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
    }
}

fn draw_version_status(state: &mut AppState, ui: &mut egui::Ui) {
    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    if state.update_install.is_some() {
        ui.label(egui::RichText::new("Installing update...").color(colors::TEXT_DISABLED));
        return;
    }
    if state.pending_update.is_some() {
        let response = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{current} - New version available! Click to update"
                    ))
                    .color(colors::HOVER),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            state.start_update_install();
        }
    } else {
        ui.label(egui::RichText::new(current).color(colors::TEXT_DISABLED));
    }
}

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let gained_focus = ctx.input(|i| state.focus_refresh.update(i.focused, Instant::now()));
    if gained_focus {
        state.refresh_all_asset_types();
        state.rebuild_prod_folder_cache();
        state.rebuild_local_folder_cache();
        state.start_validation_for_visible_assets();
    }

    egui::TopBottomPanel::top("menu").show(ctx, |ui| {
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(0.0, 4.0))
            .show(ui, |ui| menu::draw(state, ui));
    });
    state.start_validation_if_visible_scope_changed();
    dialogs::token_prompt(state, ctx);
    dialogs::settings(state, ctx);
    dialogs::draw(state, ctx);
    draw_update_prompt(state, ctx);
    draw_create_prod_folder_prompt(state, ctx);
    draw_delete_local_folder_prompt(state, ctx);
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
        || !state.plan_jobs.is_empty()
        || state.prod_cache_rx.is_some()
        || !state.verifications.is_empty()
        || state.validation_job.is_some()
        || state.update_check_rx.is_some()
        || state.update_install.is_some()
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
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
        .default_width(620.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!("A new PHASE version is available: {}", update.tag));
            ui.add_space(8.0);
            ui.label("Release notes:");
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut notes)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .interactive(false),
                    );
                });
            ui.add_space(8.0);
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
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{mpsc::channel, Arc, Mutex};
    use std::time::Instant;

    use egui::{epaint::ClippedShape, Pos2, RawInput, Rect, Vec2};

    use crate::config::Config;
    use crate::copy::job::JobProgress;
    use crate::copy::plan::{Direction, Plan};
    use crate::notion::{Asset, AssetList, AssetStatus, StatusGroup};

    fn test_state() -> super::AppState {
        let current_type = super::AssetType::Hdris;
        let mut assets_by_type = HashMap::new();
        assets_by_type.insert(
            current_type,
            super::AssetListState::Loaded(AssetList {
                assets: Vec::new(),
                statuses: Vec::new(),
            }),
        );

        super::AppState {
            config: Config::default(),
            current_type,
            selected_types: vec![current_type],
            selected_status_groups: StatusGroup::default_filter(),
            author_filter: String::new(),
            author_filters: Vec::new(),
            assets_by_type,
            error_banner: None,
            jobs: HashMap::new(),
            plan_jobs: HashMap::new(),
            verifications: HashMap::new(),
            status_updates: HashMap::new(),
            title_renames: HashMap::new(),
            notion_rx: HashMap::new(),
            pending_conflict: None,
            pending_verification_failure: None,
            pending_prod_folder_create: None,
            pending_local_folder_delete: None,
            row_toasts: HashMap::new(),
            published_assets: crate::polyhaven::PublishedAssets::default(),
            published_rx: None,
            refreshing_published: false,
            token_prompt_open: false,
            token_input: String::new(),
            auth_login: None,
            auth_rx: None,
            settings_open: false,
            settings_local_root_input: String::new(),
            settings_skip_pull_raw_tif_if_many_work_tifs: true,
            settings_open_notion_links_in_desktop_app: false,
            refreshing: HashSet::new(),
            pending_notion: HashMap::new(),
            cursor_moved_in_table_at: None,
            focus_refresh: super::focus_refresh::State::default(),
            prod_folder_cache: HashMap::new(),
            prod_cache_rx: None,
            local_folder_cache: HashMap::new(),
            dismissed_warning_keys: HashSet::new(),
            validation_results: HashMap::new(),
            validation_job: None,
            visible_validation_scope: super::VisibleValidationScope::default(),
            update_check_rx: None,
            pending_update: None,
            update_dialog_open: false,
            update_install: None,
            transfer_estimates: HashMap::new(),
            transfer_estimate_jobs: HashMap::new(),
            search_query: String::new(),
        }
    }

    fn render_text_shapes(state: &mut super::AppState) -> Vec<(String, Pos2)> {
        let ctx = egui::Context::default();
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
                ..Default::default()
            },
            |ctx| super::draw(state, ctx),
        );
        let mut texts = Vec::new();
        collect_text_shapes(&output.shapes, &mut texts);
        texts
    }

    fn collect_text_shapes(shapes: &[ClippedShape], texts: &mut Vec<(String, Pos2)>) {
        for clipped in shapes {
            collect_shape_text(&clipped.shape, texts);
        }
    }

    fn collect_shape_text(shape: &egui::epaint::Shape, texts: &mut Vec<(String, Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                texts.push((text.galley.job.text.clone(), text.pos));
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_shape_text(shape, texts);
                }
            }

            _ => {}
        }
    }

    fn asset(slug: &str, author: &str, group: StatusGroup) -> Asset {
        Asset {
            page_id: slug.into(),
            slug: slug.into(),
            author: author.into(),
            url: String::new(),
            status: Some(AssetStatus {
                id: format!("{slug}-status"),
                name: group.label().into(),
                color: "default".into(),
                group,
                sort_order: 0,
            }),
        }
    }

    #[test]
    fn prod_folder_structure_creates_expected_subfolders() {
        let temp = tempfile::tempdir().unwrap();

        super::create_prod_folder_structure_at(temp.path(), super::AssetType::Hdris).unwrap();

        assert!(temp.path().join("raw").is_dir());
        assert!(temp.path().join("staging").is_dir());
        assert!(temp.path().join("work").is_dir());
    }

    #[test]
    fn all_authors_persistence_does_not_fall_back_to_legacy_per_type_filter() {
        let mut config = Config::default();
        config.last_asset_types = vec!["HDRIs".into()];
        config.last_author_filter = String::new();
        config.last_author_filters = Vec::new();
        config
            .last_filters
            .insert("HDRIs".into(), "Stale Author".into());

        let state = super::AppState::new(config);

        assert!(state.author_filter.is_empty());
        assert!(state.author_filters.is_empty());
    }

    #[test]
    fn selected_asset_types_combine_saved_author_filters_per_type() {
        let mut config = Config::default();
        config.last_asset_types = vec!["HDRIs".into(), "Textures".into()];
        config
            .last_author_filters_by_type
            .insert("HDRIs".into(), vec!["Dario".into()]);
        config
            .last_author_filters_by_type
            .insert("Textures".into(), vec!["Charlotte".into()]);

        let state = super::AppState::new(config);

        assert_eq!(
            state.author_filters,
            vec!["Charlotte".to_string(), "Dario".to_string()]
        );
        assert_eq!(state.author_filter, "Charlotte");
    }

    #[test]
    fn persisting_author_filters_partitions_selection_by_asset_type() {
        let mut state = test_state();
        state.selected_types = vec![super::AssetType::Hdris, super::AssetType::Textures];
        state.author_filters = vec!["Charlotte".into(), "Dario".into()];
        state.assets_by_type.insert(
            super::AssetType::Hdris,
            super::AssetListState::Loaded(AssetList {
                assets: vec![asset("hdri", "Dario", StatusGroup::InProgress)],
                statuses: Vec::new(),
            }),
        );
        state.assets_by_type.insert(
            super::AssetType::Textures,
            super::AssetListState::Loaded(AssetList {
                assets: vec![asset("texture", "Charlotte", StatusGroup::InProgress)],
                statuses: Vec::new(),
            }),
        );

        state.persist_author_filters_for_selected_types();

        assert_eq!(
            state.config.last_author_filters_by_type.get("HDRIs"),
            Some(&vec!["Dario".to_string()])
        );
        assert_eq!(
            state.config.last_author_filters_by_type.get("Textures"),
            Some(&vec!["Charlotte".to_string()])
        );
    }

    #[test]
    fn update_check_runs_only_once_per_day() {
        assert!(super::should_check_for_update(None, 42));
        assert!(super::should_check_for_update(Some(41), 42));
        assert!(!super::should_check_for_update(Some(42), 42));
    }

    #[test]
    fn transfer_estimate_uses_copy_plan_total_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state();
        state.config.local_root = temp.path().join("local");
        state.config.prod_root = temp.path().join("prod");
        let key = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "asset".into(),
        };
        let local_asset = state.local_root_for(key.asset_type).join(&key.slug);
        std::fs::create_dir_all(local_asset.join("staging")).unwrap();
        std::fs::create_dir_all(state.prod_root_for(key.asset_type).join(&key.slug)).unwrap();
        std::fs::write(local_asset.join("staging").join("file.bin"), [1u8, 2, 3, 4]).unwrap();

        state.start_transfer_estimate(&key, Direction::Push);
        for _ in 0..50 {
            state.pump();
            if state.transfer_estimate_jobs.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(
            state.transfer_estimates.get(&(key, Direction::Push)),
            Some(&4)
        );
    }

    #[test]
    fn active_file_action_status_includes_direction_asset_and_file() {
        let key = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "foo".into(),
        };

        assert_eq!(
            super::format_active_file_action(&key, Direction::Pull, Some("bar.xyz")),
            "Downloading bar.xyz from Prod"
        );
        assert_eq!(
            super::format_active_file_action(&key, Direction::Push, Some("bar.xyz")),
            "Uploading bar.xyz to Prod"
        );
    }

    #[test]
    fn active_file_action_abbreviates_paths_to_two_parents_with_windows_separators() {
        let key = super::RowKey {
            asset_type: super::AssetType::Textures,
            slug: "foo".into(),
        };

        assert_eq!(
            super::format_active_file_action(
                &key,
                Direction::Pull,
                Some("staging/textures/foobar.exr")
            ),
            "Downloading staging\\textures\\foobar.exr from Prod"
        );
        assert_eq!(
            super::format_active_file_action(
                &key,
                Direction::Pull,
                Some("staging/textures/foo/bar.exr")
            ),
            "Downloading ...textures\\foo\\bar.exr from Prod"
        );
    }

    #[test]
    fn visible_validation_keys_follow_asset_type_author_and_status_filters() {
        let mut state = test_state();
        state.assets_by_type.insert(
            super::AssetType::Hdris,
            super::AssetListState::Loaded(AssetList {
                assets: vec![
                    asset("visible_hdri", "Alice", StatusGroup::InProgress),
                    asset("wrong_author", "Bob", StatusGroup::InProgress),
                    asset("wrong_status", "Alice", StatusGroup::Complete),
                ],
                statuses: Vec::new(),
            }),
        );
        state.assets_by_type.insert(
            super::AssetType::Textures,
            super::AssetListState::Loaded(AssetList {
                assets: vec![asset("hidden_texture", "Alice", StatusGroup::InProgress)],
                statuses: Vec::new(),
            }),
        );
        state.selected_types = vec![super::AssetType::Hdris];
        state.selected_status_groups = vec![StatusGroup::InProgress];
        state.author_filters = vec!["Alice".into()];

        let keys = state.visible_validation_scope_snapshot().keys;

        assert_eq!(
            keys,
            vec![super::RowKey {
                asset_type: super::AssetType::Hdris,
                slug: "visible_hdri".into()
            }]
        );
    }

    #[test]
    fn folder_caches_rebuild_only_for_visible_assets() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = test_state();
        state.config.prod_root = temp.path().join("prod");
        state.config.local_root = temp.path().join("local");
        std::fs::create_dir_all(state.config.prod_root.join("HDRIs").join("visible_hdri")).unwrap();
        std::fs::create_dir_all(state.config.prod_root.join("HDRIs").join("hidden_hdri")).unwrap();
        std::fs::create_dir_all(state.config.local_root.join("HDRIs").join("visible_hdri")).unwrap();
        std::fs::create_dir_all(state.config.local_root.join("HDRIs").join("hidden_hdri")).unwrap();
        state.assets_by_type.insert(
            super::AssetType::Hdris,
            super::AssetListState::Loaded(AssetList {
                assets: vec![
                    asset("visible_hdri", "Alice", StatusGroup::InProgress),
                    asset("hidden_hdri", "Bob", StatusGroup::InProgress),
                ],
                statuses: Vec::new(),
            }),
        );
        state.selected_types = vec![super::AssetType::Hdris];
        state.selected_status_groups = vec![StatusGroup::InProgress];
        state.author_filters = vec!["Alice".into()];

        state.rebuild_prod_folder_cache();
        for _ in 0..50 {
            state.pump();
            if state.prod_cache_rx.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        state.rebuild_local_folder_cache();

        let visible = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "visible_hdri".into(),
        };
        let hidden = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "hidden_hdri".into(),
        };
        assert_eq!(state.prod_folder_cache.get(&visible), Some(&true));
        assert!(!state.prod_folder_cache.contains_key(&hidden));
        assert_eq!(state.local_folder_cache.get(&visible), Some(&true));
        assert!(!state.local_folder_cache.contains_key(&hidden));
    }

    #[test]
    fn starting_visible_validation_keeps_cached_results_in_memory() {
        let mut state = test_state();
        let stale_key = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "previously_visible".into(),
        };
        state.validation_results.insert(
            stale_key.clone(),
            vec![crate::validation::Finding {
                severity: crate::validation::Severity::Warning,
                text: "Cached warning".into(),
                dismiss_id: None,
            }],
        );

        state.start_validation_for_visible_assets();

        assert!(state.validation_results.contains_key(&stale_key));
    }

    #[test]
    fn visible_validation_scope_changes_when_filters_change_even_if_keys_do_not() {
        let mut state = test_state();
        state.assets_by_type.insert(
            super::AssetType::Hdris,
            super::AssetListState::Loaded(AssetList {
                assets: vec![asset("visible_hdri", "Alice", StatusGroup::InProgress)],
                statuses: Vec::new(),
            }),
        );
        state.selected_types = vec![super::AssetType::Hdris];
        state.selected_status_groups = vec![StatusGroup::InProgress];
        state.author_filters = Vec::new();
        let all_authors_scope = state.visible_validation_scope_snapshot();

        state.author_filters = vec!["Alice".into()];
        let alice_scope = state.visible_validation_scope_snapshot();

        assert_ne!(alice_scope, all_authors_scope);
        assert_eq!(
            alice_scope.keys,
            vec![super::RowKey {
                asset_type: super::AssetType::Hdris,
                slug: "visible_hdri".into()
            }]
        );
    }

    #[test]
    fn error_message_renders_in_bottom_status_bar_area() {
        let mut state = test_state();
        state.error_banner = Some("Cannot create Prod folder".into());

        let texts = render_text_shapes(&mut state);
        let y = texts
            .iter()
            .find_map(|(text, pos)| (text == "Cannot create Prod folder").then_some(pos.y))
            .expect("error text should be rendered");

        assert!(
            y > 600.0,
            "expected error text near the bottom status bar, got y={y}"
        );
    }

    #[test]
    fn error_message_replaces_active_progress_text_while_visible() {
        let mut state = test_state();
        state.error_banner = Some("Cannot create Prod folder".into());
        let key = super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "foo".into(),
        };
        let progress = Arc::new(JobProgress::default());
        *progress.current_file.lock().unwrap() = Some("bar.xyz".into());
        let (_tx, rx) = channel();
        state.jobs.insert(
            key.clone(),
            super::RowJob {
                direction: Direction::Push,
                plan: Plan {
                    direction: Direction::Push,
                    src_root: std::path::PathBuf::from(r"C:\src"),
                    dst_root: std::path::PathBuf::from(r"C:\dst"),
                    files: Vec::new(),
                    total_bytes_to_copy: 0,
                },
                progress,
                rx,
                started_at: Instant::now(),
                message: Arc::new(Mutex::new(String::new())),
            },
        );

        let texts = render_text_shapes(&mut state);

        assert!(texts
            .iter()
            .any(|(text, _)| text == "Cannot create Prod folder"));
        assert!(!texts
            .iter()
            .any(|(text, _)| { text.contains("Uploading HDRIs/foo/bar.xyz to Prod") }));
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

pub fn list_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/list.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "list_icon", "list.svg"))
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

pub fn gear_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/gear-fill.svg");
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| load_svg_texture(ctx, BYTES, "gear_icon", "gear-fill.svg"))
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
