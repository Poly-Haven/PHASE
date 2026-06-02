use anyhow::{Context, Result};
use filetime::FileTime;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    New,
    Overwrite,
    Conflict { dest_newer: bool },
    Identical,
}

#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub rel_path: PathBuf,
    pub src_abs: PathBuf,
    pub dst_abs: PathBuf,
    pub size: u64,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Pull,
    Push,
}

const PULL_EXCLUDED_EXT: &[&str] = &[
    "tif", "tiff", "nef", "cr2", "cr3", "arw", "rw2", "orf", "raf", "dng",
];
const MTIME_TOLERANCE_SECS: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullFilterMode {
    AlwaysSkipRawAndTif,
    SkipRawAndTifWhenWorkTifsExceed { threshold: usize },
    None,
}

pub fn is_excluded_for_pull(file_name: &str) -> bool {
    let Some(ext) = Path::new(file_name).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext_lower = ext.to_ascii_lowercase();
    PULL_EXCLUDED_EXT.iter().any(|e| *e == ext_lower)
}

pub fn classify(
    src_size: u64,
    src_mtime: i64,
    dst_size: Option<u64>,
    dst_mtime: Option<i64>,
) -> Action {
    let (Some(dsz), Some(dmt)) = (dst_size, dst_mtime) else {
        return Action::New;
    };
    if dsz == src_size && (src_mtime - dmt).abs() <= MTIME_TOLERANCE_SECS {
        return Action::Identical;
    }
    let delta = src_mtime - dmt;
    if delta > MTIME_TOLERANCE_SECS {
        Action::Overwrite
    } else if delta < -MTIME_TOLERANCE_SECS {
        Action::Conflict { dest_newer: true }
    } else {
        Action::Overwrite
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub direction: Direction,
    pub src_root: PathBuf,
    pub dst_root: PathBuf,
    pub files: Vec<PlannedFile>,
    pub total_bytes_to_copy: u64,
}

impl Plan {
    pub fn conflicts(&self) -> Vec<&PlannedFile> {
        self.files
            .iter()
            .filter(|f| matches!(f.action, Action::Conflict { .. }))
            .collect()
    }
}

/// Walk `src_root`, classify each file against `dst_root`, return the plan.
pub fn build_plan(direction: Direction, src_root: &Path, dst_root: &Path) -> Result<Plan> {
    build_plan_with_pull_filter(
        direction,
        src_root,
        dst_root,
        PullFilterMode::AlwaysSkipRawAndTif,
    )
}

pub fn build_plan_with_pull_filter(
    direction: Direction,
    src_root: &Path,
    dst_root: &Path,
    pull_filter: PullFilterMode,
) -> Result<Plan> {
    let mut files = Vec::new();
    let mut total = 0u64;
    let skip_pull_excluded = direction == Direction::Pull
        && match pull_filter {
            PullFilterMode::AlwaysSkipRawAndTif => true,
            PullFilterMode::None => false,
            PullFilterMode::SkipRawAndTifWhenWorkTifsExceed { threshold } => {
                work_tif_count_exceeds(src_root, threshold)?
            }
        };

    for entry in WalkDir::new(src_root).follow_links(false) {
        let entry = entry.with_context(|| format!("walking {}", src_root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.ends_with(".partial") {
            continue;
        }
        if skip_pull_excluded && is_excluded_for_pull(&file_name) {
            continue;
        }

        let src_abs = entry.path().to_path_buf();
        let rel = src_abs.strip_prefix(src_root).unwrap().to_path_buf();
        let dst_abs = dst_root.join(&rel);

        let src_md =
            fs::metadata(&src_abs).with_context(|| format!("stat {}", src_abs.display()))?;
        let src_size = src_md.len();
        let src_mtime = FileTime::from_last_modification_time(&src_md).unix_seconds();

        let (dst_size, dst_mtime) = match fs::metadata(&dst_abs) {
            Ok(m) => (
                Some(m.len()),
                Some(FileTime::from_last_modification_time(&m).unix_seconds()),
            ),
            Err(_) => (None, None),
        };

        let action = classify(src_size, src_mtime, dst_size, dst_mtime);
        if matches!(action, Action::New | Action::Overwrite) {
            total += src_size;
        }
        files.push(PlannedFile {
            rel_path: rel,
            src_abs,
            dst_abs,
            size: src_size,
            action,
        });
    }

    Ok(Plan {
        direction,
        src_root: src_root.to_path_buf(),
        dst_root: dst_root.to_path_buf(),
        files,
        total_bytes_to_copy: total,
    })
}

fn work_tif_count_exceeds(src_root: &Path, threshold: usize) -> Result<bool> {
    let work_root = src_root.join("work");
    if !work_root.is_dir() {
        return Ok(false);
    }
    let mut count = 0usize;
    for entry in WalkDir::new(&work_root).follow_links(false) {
        let entry = entry.with_context(|| format!("walking {}", work_root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let is_tif = entry
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                let ext = ext.to_ascii_lowercase();
                ext == "tif" || ext == "tiff"
            })
            .unwrap_or(false);
        if is_tif {
            count += 1;
            if count > threshold {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
