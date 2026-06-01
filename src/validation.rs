use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use walkdir::WalkDir;

use crate::notion::AssetStatus;
use crate::ui::{AssetType, RowKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub key: RowKey,
    pub status: Option<AssetStatus>,
    pub local_root: PathBuf,
    pub prod_root: PathBuf,
}

pub enum Msg {
    RowValidated { key: RowKey, findings: Vec<Finding> },
    Finished,
}

pub fn spawn(requests: Vec<Request>, tx: Sender<Msg>) {
    thread::spawn(move || {
        for request in requests {
            let findings = validate_asset(
                request.key.asset_type,
                &request.key.slug,
                request.status.as_ref(),
                &request.local_root,
                &request.prod_root,
            );
            if tx
                .send(Msg::RowValidated {
                    key: request.key,
                    findings,
                })
                .is_err()
            {
                return;
            }
        }
        let _ = tx.send(Msg::Finished);
    });
}

pub fn validate_asset(
    asset_type: AssetType,
    slug: &str,
    status: Option<&AssetStatus>,
    local_root: &Path,
    prod_root: &Path,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    if local_root.is_dir() {
        if let Some(finding) = validate_root_entries(asset_type, local_root) {
            findings.push(finding);
        }
    }

    if local_root.is_dir() && prod_root.is_dir() && local_is_newer_or_extra(local_root, prod_root) {
        findings.push(Finding {
            severity: if is_needs_review(status) {
                Severity::Warning
            } else {
                Severity::Info
            },
            text: "Local files newer than Prod. Push?".into(),
        });
    }

    if is_needs_review(status) && prod_root.is_dir() {
        findings.extend(validate_needs_review_requirements(
            asset_type, slug, prod_root,
        ));
    }

    findings
}

fn validate_root_entries(asset_type: AssetType, local_root: &Path) -> Option<Finding> {
    let primary = match asset_type {
        AssetType::Hdris | AssetType::Textures => "raw",
    };
    let mut unexpected = Vec::new();
    let read_dir = std::fs::read_dir(local_root).ok()?;
    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !matches!(name.as_str(), "work" | "staging") && name != primary {
                unexpected.push(name);
            }
        } else if !is_harmless_root_file(entry.file_name().as_os_str()) {
            unexpected.push(name);
        }
    }
    if unexpected.is_empty() {
        None
    } else {
        unexpected.sort_unstable();
        Some(Finding {
            severity: Severity::Error,
            text: format!("Unexpected root entries: {}", unexpected.join(", ")),
        })
    }
}

fn local_is_newer_or_extra(local_root: &Path, prod_root: &Path) -> bool {
    for entry in WalkDir::new(local_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(local_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        if is_harmless_root_file(entry.file_name()) {
            continue;
        }
        let prod_path = prod_root.join(rel);
        if !prod_path.is_file() {
            return true;
        }
        let local_mtime = match std::fs::metadata(entry.path()).and_then(|meta| meta.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };
        let prod_mtime = match std::fs::metadata(&prod_path).and_then(|meta| meta.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };
        if local_mtime > prod_mtime {
            return true;
        }
    }
    false
}

fn validate_needs_review_requirements(
    asset_type: AssetType,
    slug: &str,
    prod_root: &Path,
) -> Vec<Finding> {
    let staging = prod_root.join("staging");
    let mut findings = Vec::new();
    match asset_type {
        AssetType::Hdris => {
            if !staging.join(format!("{slug}.exr")).is_file() {
                findings.push(Finding {
                    severity: Severity::Error,
                    text: format!("Missing /staging/{slug}.exr in Prod"),
                });
            }
            if !staging.join("colorchart.zip").is_file() {
                findings.push(Finding {
                    severity: Severity::Warning,
                    text: "Missing /staging/colorchart.zip in Prod".into(),
                });
            }
        }
        AssetType::Textures => {
            if !staging.join(format!("{slug}.blend")).is_file() {
                findings.push(Finding {
                    severity: Severity::Error,
                    text: format!("Missing /staging/{slug}.blend in Prod"),
                });
            }
            if !staging.join("textures").is_dir() {
                findings.push(Finding {
                    severity: Severity::Error,
                    text: "Missing /staging/textures in Prod".into(),
                });
            }
        }
    }
    findings
}

fn is_needs_review(status: Option<&AssetStatus>) -> bool {
    status
        .map(|status| status.name.eq_ignore_ascii_case("Needs review"))
        .unwrap_or(false)
}

fn is_harmless_root_file(name: &OsStr) -> bool {
    matches!(
        name.to_string_lossy().to_ascii_lowercase().as_str(),
        "thumbs.db" | "desktop.ini" | ".ds_store" | "ehthumbs.db"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, SystemTime};

    use tempfile::tempdir;

    use crate::notion::{AssetStatus, StatusGroup};

    use super::*;

    fn needs_review_status() -> AssetStatus {
        AssetStatus {
            id: "needs-review".into(),
            name: "Needs review".into(),
            color: "yellow".into(),
            group: StatusGroup::InProgress,
        }
    }

    fn in_progress_status() -> AssetStatus {
        AssetStatus {
            id: "in-progress".into(),
            name: "Shooting".into(),
            color: "blue".into(),
            group: StatusGroup::InProgress,
        }
    }

    #[test]
    fn unexpected_root_entries_are_reported_but_harmless_files_are_ignored() {
        let temp = tempdir().unwrap();
        let local_root = temp.path().join("local");
        let prod_root = temp.path().join("prod");
        fs::create_dir_all(local_root.join("raw")).unwrap();
        fs::create_dir_all(local_root.join("work")).unwrap();
        fs::create_dir_all(local_root.join("staging")).unwrap();
        fs::create_dir_all(local_root.join("renders")).unwrap();
        fs::write(local_root.join("Thumbs.db"), b"ignore").unwrap();
        fs::write(local_root.join("notes.txt"), b"unexpected").unwrap();

        let findings = validate_asset(
            AssetType::Hdris,
            "test_slug",
            Some(&in_progress_status()),
            &local_root,
            &prod_root,
        );

        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Error
                && finding.text.contains("renders")
                && finding.text.contains("notes.txt")
                && !finding.text.contains("Thumbs.db")
        }));
    }

    #[test]
    fn newer_local_files_are_warning_for_needs_review() {
        let temp = tempdir().unwrap();
        let local_root = temp.path().join("local");
        let prod_root = temp.path().join("prod");
        fs::create_dir_all(local_root.join("work")).unwrap();
        fs::create_dir_all(prod_root.join("work")).unwrap();
        let local_file = local_root.join("work").join("asset.txt");
        let prod_file = prod_root.join("work").join("asset.txt");
        fs::write(&local_file, b"newer").unwrap();
        fs::write(&prod_file, b"older").unwrap();
        let local_time = filetime::FileTime::from_system_time(SystemTime::now());
        let prod_time =
            filetime::FileTime::from_system_time(SystemTime::now() - Duration::from_secs(60));
        filetime::set_file_mtime(&local_file, local_time).unwrap();
        filetime::set_file_mtime(&prod_file, prod_time).unwrap();

        let findings = validate_asset(
            AssetType::Hdris,
            "test_slug",
            Some(&needs_review_status()),
            &local_root,
            &prod_root,
        );

        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Warning
                && finding.text == "Local files newer than Prod. Push?"
        }));
    }

    #[test]
    fn newer_local_files_are_info_when_not_needs_review() {
        let temp = tempdir().unwrap();
        let local_root = temp.path().join("local");
        let prod_root = temp.path().join("prod");
        fs::create_dir_all(local_root.join("work")).unwrap();
        fs::create_dir_all(prod_root.join("work")).unwrap();
        fs::write(local_root.join("work").join("asset.txt"), b"only-local").unwrap();

        let findings = validate_asset(
            AssetType::Hdris,
            "test_slug",
            Some(&in_progress_status()),
            &local_root,
            &prod_root,
        );

        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Info
                && finding.text == "Local files newer than Prod. Push?"
        }));
    }

    #[test]
    fn needs_review_hdris_require_exr_and_warn_about_colorchart() {
        let temp = tempdir().unwrap();
        let local_root = temp.path().join("local");
        let prod_root = temp.path().join("prod");
        fs::create_dir_all(prod_root.join("staging")).unwrap();

        let findings = validate_asset(
            AssetType::Hdris,
            "sunny_field",
            Some(&needs_review_status()),
            &local_root,
            &prod_root,
        );

        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Error
                && finding.text == "Missing /staging/sunny_field.exr in Prod"
        }));
        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Warning
                && finding.text == "Missing /staging/colorchart.zip in Prod"
        }));
    }

    #[test]
    fn needs_review_textures_require_blend_and_textures_folder() {
        let temp = tempdir().unwrap();
        let local_root = temp.path().join("local");
        let prod_root = temp.path().join("prod");
        fs::create_dir_all(prod_root.join("staging")).unwrap();

        let findings = validate_asset(
            AssetType::Textures,
            "forest_floor",
            Some(&needs_review_status()),
            &local_root,
            &prod_root,
        );

        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Error
                && finding.text == "Missing /staging/forest_floor.blend in Prod"
        }));
        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Error
                && finding.text == "Missing /staging/textures in Prod"
        }));
    }
}
