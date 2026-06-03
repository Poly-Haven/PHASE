use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use super::{
    AppState, AssetListState, AssetType, ConflictChoice, PlanJob, RowJob, RowKey, StatusUpdateJob,
    TitleRenameJob,
};
use crate::auth::AuthTokens;
use crate::copy::job::JobProgress;
use crate::copy::plan::{build_plan_with_pull_filter, Action, Direction, Plan, PullFilterMode};
use crate::notion::{AssetStatus, StatusOption};

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

pub(super) fn spawn_copy_job(state: &mut AppState, key: RowKey, direction: Direction, plan: Plan) {
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

pub(super) fn set_asset_status(
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

pub(super) fn create_prod_folder_structure_at(
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
