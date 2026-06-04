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
        logged_in_identity: None,
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
    for _ in 0..50 {
        state.pump();
        if state.validation_job.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("validation job did not finish");
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

    state.start_transfer_estimate(&key, Direction::Push, true);
    for _ in 0..50 {
        state.pump();
        if state.transfer_estimate_jobs.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(
        state.transfer_estimates.get(&(key, Direction::Push)),
        Some(&super::ActionPreview {
            file_count: 1,
            bytes: 4
        })
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
        .any(|(text, _)| text == "Logged in as Ada [admin]"));
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
    state.validation_results.insert(errored.clone(), error_finding.clone());
    state.validation_results.insert(clean.clone(), Vec::new());
    state
        .validation_results
        .insert(errored_no_folder.clone(), error_finding);
    state.prod_folder_cache.insert(errored.clone(), true);
    state.prod_folder_cache.insert(clean.clone(), true);
    state.prod_folder_cache.insert(errored_no_folder.clone(), false);

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
