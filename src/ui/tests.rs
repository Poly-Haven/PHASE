use std::collections::{HashMap, HashSet};
use std::sync::{mpsc::channel, Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use egui::{epaint::ClippedShape, Color32, Pos2, RawInput, Rect, Vec2};

use crate::config::Config;
use crate::copy::job::JobProgress;
use crate::copy::plan::{Action, Direction, Plan, PlannedFile};
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
        transfer_file_list_dialog: None,
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
        logged_in_identity: None,
        settings_open: false,
        settings_local_root_input: String::new(),
        settings_open_notion_links_in_desktop_app: false,
        refreshing: HashSet::new(),
        pending_notion: HashMap::new(),
        cursor_moved_in_table_at: None,
        focus_refresh: super::focus_refresh::State::default(),
        prod_folder_cache: HashMap::new(),
        prod_cache_rx: None,
        thumbnail_cache_root: std::path::PathBuf::from("thumbnail-cache-test"),
        thumbnail_revisions: HashMap::new(),
        thumbnail_jobs: HashMap::new(),
        thumbnail_previews: HashMap::new(),
        thumbnail_cleanup_rx: None,
        local_folder_cache: HashMap::new(),
        dismissed_warning_keys: HashSet::new(),
        validation_results: HashMap::new(),
        validation_job: None,
        visible_validation_scope: super::VisibleValidationScope::default(),
        update_check: None,
        pending_update: None,
        version_notice: None,
        update_dialog_open: false,
        update_install: None,
        transfer_estimates: HashMap::new(),
        transfer_estimate_jobs: HashMap::new(),
        script_jobs: HashMap::new(),
        script_queue: std::collections::VecDeque::new(),
        script_results: HashMap::new(),
        script_output_dialog: None,
        search_query: String::new(),
        file_watcher: None,
        last_activity_at: Instant::now(),
        watcher_was_focused: false,
        last_watch_mode: super::file_watcher::WatchMode::RealTime,
        next_poll_at: Instant::now(),
        watch_dirty: true,
        watch_pending: HashMap::new(),
        pending_validation_keys: HashSet::new(),
    }
}

fn render_text_shapes(state: &mut super::AppState) -> Vec<(String, Pos2, Option<Color32>)> {
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

fn collect_text_shapes(shapes: &[ClippedShape], texts: &mut Vec<(String, Pos2, Option<Color32>)>) {
    for clipped in shapes {
        collect_shape_text(&clipped.shape, texts);
    }
}

fn collect_shape_text(
    shape: &egui::epaint::Shape,
    texts: &mut Vec<(String, Pos2, Option<Color32>)>,
) {
    match shape {
        egui::epaint::Shape::Text(text) => {
            texts.push((
                text.galley.job.text.clone(),
                text.pos,
                text.override_text_color.or(Some(text.fallback_color)),
            ));
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

fn needs_review_asset(slug: &str, author: &str) -> Asset {
    Asset {
        page_id: slug.into(),
        slug: slug.into(),
        author: author.into(),
        url: String::new(),
        status: Some(AssetStatus {
            id: format!("{slug}-needs-review"),
            name: "Needs review".into(),
            color: "yellow".into(),
            group: StatusGroup::InProgress,
            sort_order: 1,
        }),
    }
}

fn pump_validation(state: &mut super::AppState) {
    let ctx = egui::Context::default();
    for _ in 0..50 {
        state.pump(&ctx);
        if state.validation_job.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("validation job did not finish");
}

fn wait_for_thumbnail_job(state: &super::AppState, key: &super::RowKey) {
    let completion = state
        .thumbnail_jobs
        .get(key)
        .expect("expected thumbnail job")
        .completed
        .clone();
    let (lock, cvar) = &*completion;
    let finished = lock.lock().unwrap();
    let (_finished, result) = cvar
        .wait_timeout_while(finished, std::time::Duration::from_secs(5), |done| !*done)
        .unwrap();
    assert!(!result.timed_out(), "thumbnail job did not finish");
}

struct ConfigBackup {
    _lock: MutexGuard<'static, ()>,
    config_path: Option<std::path::PathBuf>,
    config_bytes: Option<Vec<u8>>,
    backup_path: Option<std::path::PathBuf>,
    backup_bytes: Option<Vec<u8>>,
}

impl ConfigBackup {
    fn capture() -> Self {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        let guard = lock.lock().unwrap();
        let config_path = crate::config::config_path().ok();
        let backup_path = config_path.as_ref().map(|path| {
            let mut os = path.as_os_str().to_owned();
            os.push(".bak");
            std::path::PathBuf::from(os)
        });
        let config_bytes = config_path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok());
        let backup_bytes = backup_path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok());
        Self {
            _lock: guard,
            config_path,
            config_bytes,
            backup_path,
            backup_bytes,
        }
    }
}

impl Drop for ConfigBackup {
    fn drop(&mut self) {
        if let Some(path) = &self.config_path {
            match &self.config_bytes {
                Some(bytes) => {
                    let _ = std::fs::write(path, bytes);
                }
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        if let Some(path) = &self.backup_path {
            match &self.backup_bytes {
                Some(bytes) => {
                    let _ = std::fs::write(path, bytes);
                }
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}

#[test]
fn prod_folder_structure_creates_expected_subfolders() {
    let temp = tempfile::tempdir().unwrap();

    super::jobs::create_prod_folder_structure_at(temp.path(), super::AssetType::Hdris).unwrap();

    assert!(temp.path().join("raw").is_dir());
    assert!(temp.path().join("staging").is_dir());
    assert!(temp.path().join("work").is_dir());
}

#[test]
fn all_authors_persistence_does_not_fall_back_to_legacy_per_type_filter() {
    let _config_backup = ConfigBackup::capture();
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
    let _config_backup = ConfigBackup::capture();
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
fn force_update_check_bypasses_the_daily_gate() {
    assert!(!super::should_force_update_check(false, Some(42), 42));
    assert!(super::should_force_update_check(true, Some(42), 42));
}

#[test]
fn version_notice_expires_after_ten_seconds() {
    let mut state = test_state();
    let now = Instant::now();
    state.version_notice = Some(super::VersionNotice {
        message: "You already have the latest version".into(),
        expires_at: now + std::time::Duration::from_secs(10),
    });

    state.clear_expired_version_notice(now + std::time::Duration::from_secs(9));
    assert!(state.version_notice.is_some());

    state.clear_expired_version_notice(now + std::time::Duration::from_secs(10));
    assert!(state.version_notice.is_none());
}

#[test]
fn manual_update_check_with_no_release_shows_latest_notice() {
    let mut state = test_state();
    let (tx, rx) = channel();
    tx.send(Ok(None)).unwrap();
    state.update_check = Some(super::UpdateCheckJob {
        rx,
        show_latest_notice_on_none: true,
    });

    state.pump(&egui::Context::default());

    assert_eq!(
        state
            .version_notice
            .as_ref()
            .map(|notice| notice.message.as_str()),
        Some("You already have the latest version")
    );
}

#[test]
fn version_label_renders_grey() {
    let mut state = test_state();
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));

    let texts = render_text_shapes(&mut state);
    let color = texts
        .into_iter()
        .find(|(text, _, _)| text == &version)
        .expect("expected version label in status bar");

    let Some(color) = color.2 else {
        panic!("expected version label to have a visible color");
    };
    assert_eq!(color.r(), color.g());
    assert_eq!(color.g(), color.b());
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

    state.start_transfer_estimate(&key, super::TransferAction::PushAll, true);
    for _ in 0..50 {
        state.pump(&egui::Context::default());
        if state.transfer_estimate_jobs.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(
        state
            .transfer_estimates
            .get(&(key, super::TransferAction::PushAll)),
        Some(&super::ActionPreview {
            file_count: 1,
            bytes: 4
        })
    );
}

#[test]
fn transfer_estimate_uses_staging_only_variant() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = test_state();
    state.config.local_root = temp.path().join("local");
    state.config.prod_root = temp.path().join("prod");
    let key = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "asset".into(),
    };
    let prod_asset = state.prod_root_for(key.asset_type).join(&key.slug);
    std::fs::create_dir_all(prod_asset.join("staging")).unwrap();
    std::fs::write(prod_asset.join("staging").join("file.bin"), [9u8, 8, 7]).unwrap();

    state.start_transfer_estimate(&key, super::TransferAction::PullStagingOnly, true);
    for _ in 0..50 {
        state.pump(&egui::Context::default());
        if state.transfer_estimate_jobs.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(
        state
            .transfer_estimates
            .get(&(key, super::TransferAction::PullStagingOnly)),
        Some(&super::ActionPreview {
            file_count: 1,
            bytes: 3
        })
    );
}

#[test]
fn transfer_file_display_rows_filter_unchanged_files_and_color_reasons() {
    let plan = Plan {
        direction: Direction::Pull,
        src_root: std::path::PathBuf::from("src"),
        dst_root: std::path::PathBuf::from("dst"),
        files: vec![
            PlannedFile {
                rel_path: std::path::PathBuf::from("new.bin"),
                src_abs: std::path::PathBuf::from("src/new.bin"),
                dst_abs: std::path::PathBuf::from("dst/new.bin"),
                size: 12,
                action: Action::New,
            },
            PlannedFile {
                rel_path: std::path::PathBuf::from("updated.bin"),
                src_abs: std::path::PathBuf::from("src/updated.bin"),
                dst_abs: std::path::PathBuf::from("dst/updated.bin"),
                size: 12,
                action: Action::Overwrite,
            },
            PlannedFile {
                rel_path: std::path::PathBuf::from("conflict.bin"),
                src_abs: std::path::PathBuf::from("src/conflict.bin"),
                dst_abs: std::path::PathBuf::from("dst/conflict.bin"),
                size: 12,
                action: Action::Conflict { dest_newer: true },
            },
            PlannedFile {
                rel_path: std::path::PathBuf::from("same.bin"),
                src_abs: std::path::PathBuf::from("src/same.bin"),
                dst_abs: std::path::PathBuf::from("dst/same.bin"),
                size: 12,
                action: Action::Identical,
            },
        ],
        total_bytes_to_copy: 36,
    };

    let rows = super::transfer_file_display_rows(&plan);

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].path, "new.bin");
    assert_eq!(rows[0].reason, "New file");
    assert_eq!(rows[0].color, super::colors::STATUS_COMPLETE);
    assert_eq!(rows[1].path, "updated.bin");
    assert_eq!(rows[1].reason, "File updated");
    assert_eq!(rows[1].color, super::colors::MSG_INFO);
    assert_eq!(rows[2].path, "conflict.bin");
    assert_eq!(rows[2].reason, "Conflict, destination newer");
    assert_eq!(rows[2].color, super::colors::MSG_ERROR);
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
        state.pump(&egui::Context::default());
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
fn periodic_validation_refresh_picks_up_external_prod_changes() {
    // The activity-aware poll only re-checks prod for assets that ALREADY have a
    // validation error (and an existing prod folder), so this test starts from an
    // errored asset and confirms an external prod fix resolves it on the next poll.
    let temp = tempfile::tempdir().unwrap();
    let slug = "pansy_shell_beach_drone";
    let key = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: slug.into(),
    };
    let local_asset_root = temp.path().join("local").join("HDRIs").join(slug);
    let prod_asset_root = temp.path().join("prod").join("HDRIs").join(slug);
    std::fs::create_dir_all(&local_asset_root).unwrap();
    // Prod folder exists but staging is missing -> needs-review HDRI is in error.
    std::fs::create_dir_all(&prod_asset_root).unwrap();

    let mut state = test_state();
    state.config.local_root = temp.path().join("local");
    state.config.prod_root = temp.path().join("prod");
    state.assets_by_type.insert(
        super::AssetType::Hdris,
        super::AssetListState::Loaded(AssetList {
            assets: vec![needs_review_asset(slug, "Alice")],
            statuses: Vec::new(),
        }),
    );
    state.selected_types = vec![super::AssetType::Hdris];
    state.selected_status_groups = vec![StatusGroup::InProgress];

    state.start_validation_for_visible_assets();
    pump_validation(&mut state);
    state.update_prod_folder_cache_for(&key);

    // The asset is currently errored and therefore eligible for prod monitoring.
    let errored = state.validation_results.get(&key).unwrap();
    assert!(errored
        .iter()
        .any(|finding| finding.text == format!("Missing /staging/{slug}.exr in Prod")));
    assert!(state.error_keys_with_prod_folder().contains(&key));

    // Fix prod externally, then run a poll tick: the error should resolve.
    let staging = prod_asset_root.join("staging");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join(format!("{slug}.exr")), b"ok").unwrap();
    std::fs::write(staging.join("colorchart.zip"), b"ok").unwrap();

    state.poll_prod_error_assets();
    pump_validation(&mut state);

    assert_eq!(state.validation_results.get(&key), Some(&Vec::new()));
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
fn thumbnail_source_prefers_local_source_then_prod() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.local_root = temp.path().join("local");
    config.prod_root = temp.path().join("prod");
    let key = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "asset".into(),
    };

    let local_source = config
        .local_root
        .join("HDRIs")
        .join("asset")
        .join("staging")
        .join("renders")
        .join("primary.png");
    let prod_source = config
        .prod_root
        .join("HDRIs")
        .join("asset")
        .join("staging")
        .join("renders")
        .join("primary.png");
    std::fs::create_dir_all(local_source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(prod_source.parent().unwrap()).unwrap();
    std::fs::write(&prod_source, b"prod").unwrap();

    assert_eq!(
        super::thumbnails::thumbnail_source_path(&config, &key),
        Some(prod_source.clone())
    );

    std::fs::write(&local_source, b"local").unwrap();

    assert_eq!(
        super::thumbnails::thumbnail_source_path(&config, &key),
        Some(local_source)
    );
}

#[test]
fn thumbnail_cache_key_includes_asset_type_namespace() {
    let cache_root = tempfile::tempdir().unwrap();
    let signature = super::thumbnails::ThumbnailSignature {
        asset_type: super::AssetType::Hdris,
        slug: "asset".into(),
        source_mtime: 1_700_000_000,
        source_size: 12_345,
    };
    let other_signature = super::thumbnails::ThumbnailSignature {
        asset_type: super::AssetType::Textures,
        slug: "asset".into(),
        source_mtime: 1_700_000_000,
        source_size: 12_345,
    };

    let cache_path = super::thumbnails::thumbnail_cache_path(
        cache_root.path(),
        &signature,
        super::thumbnails::ThumbnailFormat::WebP,
    );
    let other_cache_path = super::thumbnails::thumbnail_cache_path(
        cache_root.path(),
        &other_signature,
        super::thumbnails::ThumbnailFormat::WebP,
    );
    let filename = cache_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(cache_path.starts_with(cache_root.path().join("hdris")));
    assert!(cache_path.starts_with(cache_root.path().join("hdris").join("asset")));
    assert!(other_cache_path.starts_with(cache_root.path().join("textures").join("asset")));
    assert_ne!(cache_path, other_cache_path);
    assert_eq!(filename, "1700000000-12345.webp");
    assert!(filename.ends_with(".webp"));
}

#[test]
fn thumbnail_pruning_removes_entries_older_than_60_days() {
    let cache_root = tempfile::tempdir().unwrap();
    let signature = super::thumbnails::ThumbnailSignature {
        asset_type: super::AssetType::Hdris,
        slug: "asset".into(),
        source_mtime: 1_700_000_000,
        source_size: 12_345,
    };
    let fresh = super::thumbnails::thumbnail_cache_path(
        cache_root.path(),
        &signature,
        super::thumbnails::ThumbnailFormat::WebP,
    );
    let stale = super::thumbnails::thumbnail_cache_path(
        cache_root.path(),
        &super::thumbnails::ThumbnailSignature {
            asset_type: super::AssetType::Textures,
            slug: "old".into(),
            source_mtime: 1_700_000_001,
            source_size: 9_999,
        },
        super::thumbnails::ThumbnailFormat::Png,
    );
    std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&fresh, b"fresh").unwrap();
    std::fs::write(&stale, b"stale").unwrap();
    let old_time = filetime::FileTime::from_unix_time(1_500_000_000, 0);
    filetime::set_file_mtime(&stale, old_time).unwrap();

    let removed = super::thumbnails::prune_thumbnail_cache(cache_root.path()).unwrap();

    assert_eq!(removed, 1);
    assert!(fresh.exists());
    assert!(!stale.exists());
}

#[test]
fn thumbnail_generation_replaces_stale_preview_after_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = test_state();
    state.config.local_root = temp.path().join("local");
    state.config.prod_root = temp.path().join("prod");
    state.thumbnail_cache_root = temp.path().join("thumbs");
    let key = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "asset".into(),
    };
    let source = state
        .config
        .local_root
        .join("HDRIs")
        .join("asset")
        .join("staging")
        .join("renders")
        .join("primary.png");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]))
        .save(&source)
        .unwrap();

    state.start_thumbnail_refresh_for_keys(vec![key.clone()]);
    let ctx = egui::Context::default();
    wait_for_thumbnail_job(&state, &key);
    state.pump(&ctx);

    assert!(state.thumbnail_jobs.is_empty());
    let initial_signature = state
        .thumbnail_previews
        .get(&key)
        .expect("expected initial preview")
        .signature
        .clone();
    assert!(!std::fs::read_dir(&state.thumbnail_cache_root)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .is_empty());

    image::RgbaImage::from_pixel(12, 12, image::Rgba([0, 255, 0, 255]))
        .save(&source)
        .unwrap();

    state.start_thumbnail_refresh_for_keys(vec![key.clone()]);
    wait_for_thumbnail_job(&state, &key);
    state.pump(&ctx);

    let preview = state
        .thumbnail_previews
        .get(&key)
        .expect("expected refreshed preview");
    assert_ne!(preview.signature, initial_signature);
    assert_eq!(preview.signature.asset_type, super::AssetType::Hdris);
}

#[test]
fn thumbnail_cleanup_only_touches_the_exact_asset_directory() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = test_state();
    state.config.local_root = temp.path().join("local");
    state.config.prod_root = temp.path().join("prod");
    state.thumbnail_cache_root = temp.path().join("thumbs");
    let key = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "foo".into(),
    };
    let sibling_key = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "foo-bar".into(),
    };
    let source = state
        .config
        .local_root
        .join("HDRIs")
        .join("foo")
        .join("staging")
        .join("renders")
        .join("primary.png");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]))
        .save(&source)
        .unwrap();

    let stale_signature = super::thumbnails::ThumbnailSignature {
        asset_type: super::AssetType::Hdris,
        slug: "foo".into(),
        source_mtime: 1_700_000_000,
        source_size: 1,
    };
    let sibling_signature = super::thumbnails::ThumbnailSignature {
        asset_type: super::AssetType::Hdris,
        slug: "foo-bar".into(),
        source_mtime: 1_700_000_000,
        source_size: 1,
    };
    let stale_cache = super::thumbnails::thumbnail_cache_path(
        &state.thumbnail_cache_root,
        &stale_signature,
        super::thumbnails::ThumbnailFormat::WebP,
    );
    let sibling_cache = super::thumbnails::thumbnail_cache_path(
        &state.thumbnail_cache_root,
        &sibling_signature,
        super::thumbnails::ThumbnailFormat::WebP,
    );
    std::fs::create_dir_all(stale_cache.parent().unwrap()).unwrap();
    std::fs::create_dir_all(sibling_cache.parent().unwrap()).unwrap();
    std::fs::write(&stale_cache, b"old").unwrap();
    std::fs::write(&sibling_cache, b"keep").unwrap();

    state.start_thumbnail_refresh_for_keys(vec![key.clone()]);
    let ctx = egui::Context::default();
    wait_for_thumbnail_job(&state, &key);
    state.pump(&ctx);

    assert!(sibling_cache.exists());
    assert!(state.thumbnail_previews.contains_key(&key));
    assert!(!state.thumbnail_previews.contains_key(&sibling_key));
}

#[test]
fn ui_thumbnail_refresh_logic_stays_off_the_ui_thread() {
    let mod_src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "\\src\\ui\\mod.rs")).unwrap();
    let table_src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "\\src\\ui\\table.rs"))
            .unwrap();

    assert!(!table_src.contains("ensure_thumbnail_job("));
    assert!(!mod_src.contains("thumbnail_signature_for("));
    assert!(!mod_src.contains("metadata()"));
    assert!(!mod_src.contains("modified()"));
    assert!(mod_src.contains("start_thumbnail_refresh_for_keys"));
}

#[test]
fn error_message_renders_in_bottom_status_bar_area() {
    let mut state = test_state();
    state.error_banner = Some("Cannot create Prod folder".into());

    let texts = render_text_shapes(&mut state);
    let y = texts
        .iter()
        .find_map(|(text, pos, _)| (text == "Cannot create Prod folder").then_some(pos.y))
        .expect("error text should be rendered");

    assert!(
        y > 600.0,
        "expected error text near the bottom status bar, got y={y}"
    );
}

#[test]
fn idle_status_bar_shows_logged_in_identity() {
    let mut state = test_state();
    state.logged_in_identity = Some(crate::auth::LoggedInIdentity {
        name: "Ada".into(),
        user_id: "auth0|abc123".into(),
        role: "admin".into(),
    });

    let texts = render_text_shapes(&mut state);

    assert!(texts
        .iter()
        .any(|(text, _, _)| text == "Logged in as Ada [admin]"));
}

#[test]
fn admin_role_detection_accepts_comma_separated_roles() {
    let mut state = test_state();
    state.logged_in_identity = Some(crate::auth::LoggedInIdentity {
        name: "Ada".into(),
        user_id: "auth0|abc123".into(),
        role: "Admin, Editor".into(),
    });

    assert!(state.is_admin());
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
        .any(|(text, _, _)| text == "Cannot create Prod folder"));
    assert!(!texts
        .iter()
        .any(|(text, _, _)| { text.contains("Uploading HDRIs/foo/bar.xyz to Prod") }));
}

#[test]
fn key_for_path_maps_prod_and_local_paths_case_insensitively() {
    let mut state = test_state();
    state.config.local_root = std::path::PathBuf::from("C:\\PHASE");
    state.config.prod_root = std::path::PathBuf::from("P:\\Assets");
    state.selected_types = vec![super::AssetType::Hdris, super::AssetType::Textures];

    let prod_key = state.key_for_path(
        std::path::Path::new("P:\\Assets\\HDRIs\\beach_tide_pools\\staging\\x.exr"),
        super::file_watcher::WatchSource::Prod,
    );
    assert_eq!(
        prod_key,
        Some(super::RowKey {
            asset_type: super::AssetType::Hdris,
            slug: "beach_tide_pools".into(),
        })
    );

    // Case-insensitive drive/folder matching (Windows paths vary in casing).
    let local_key = state.key_for_path(
        std::path::Path::new("c:\\phase\\Textures\\forest_floor\\work"),
        super::file_watcher::WatchSource::Local,
    );
    assert_eq!(
        local_key,
        Some(super::RowKey {
            asset_type: super::AssetType::Textures,
            slug: "forest_floor".into(),
        })
    );

    // A path outside the watched roots maps to nothing.
    assert_eq!(
        state.key_for_path(
            std::path::Path::new("D:\\Other\\HDRIs\\foo"),
            super::file_watcher::WatchSource::Prod,
        ),
        None
    );
    // The type root itself (no slug component) maps to nothing.
    assert_eq!(
        state.key_for_path(
            std::path::Path::new("P:\\Assets\\HDRIs"),
            super::file_watcher::WatchSource::Prod,
        ),
        None
    );
}

#[test]
fn error_keys_with_prod_folder_only_includes_errored_visible_assets_with_prod_folder() {
    let mut state = test_state();
    let errored = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "errored".into(),
    };
    let clean = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "clean".into(),
    };
    let errored_no_folder = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "errored_no_folder".into(),
    };
    state.assets_by_type.insert(
        super::AssetType::Hdris,
        super::AssetListState::Loaded(AssetList {
            assets: vec![
                asset("errored", "Alice", StatusGroup::InProgress),
                asset("clean", "Alice", StatusGroup::InProgress),
                asset("errored_no_folder", "Alice", StatusGroup::InProgress),
            ],
            statuses: Vec::new(),
        }),
    );
    state.selected_types = vec![super::AssetType::Hdris];
    state.selected_status_groups = vec![StatusGroup::InProgress];

    let error_finding = vec![crate::validation::Finding {
        severity: crate::validation::Severity::Error,
        text: "boom".into(),
        dismiss_id: None,
    }];
    state
        .validation_results
        .insert(errored.clone(), error_finding.clone());
    state.validation_results.insert(clean.clone(), Vec::new());
    state
        .validation_results
        .insert(errored_no_folder.clone(), error_finding);
    state.prod_folder_cache.insert(errored.clone(), true);
    state.prod_folder_cache.insert(clean.clone(), true);
    state
        .prod_folder_cache
        .insert(errored_no_folder.clone(), false);

    let keys = state.error_keys_with_prod_folder();
    assert_eq!(keys, vec![errored]);
}

#[test]
fn queue_revalidation_coalesces_while_a_job_is_running() {
    let mut state = test_state();
    let key_a = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "a".into(),
    };
    let key_b = super::RowKey {
        asset_type: super::AssetType::Hdris,
        slug: "b".into(),
    };
    // Simulate an in-flight validation job so the queue must defer.
    let (_tx, rx) = channel();
    state.validation_job = Some(super::ValidationJob { rx });

    state.queue_revalidation(vec![key_a.clone()]);
    state.queue_revalidation(vec![key_b.clone()]);

    // Nothing started a new job; both keys are queued for when the job finishes.
    assert!(state.pending_validation_keys.contains(&key_a));
    assert!(state.pending_validation_keys.contains(&key_b));
}
