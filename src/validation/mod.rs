mod dismissed;
mod local_freshness;
mod needs_review;
mod root_entries;
pub(crate) mod workers;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use crate::notion::AssetStatus;
use crate::ui::{AssetType, RowKey};

pub use dismissed::{dismissal_key, load_dismissed_warning_keys, save_dismissed_warning_keys};
pub use workers::spawn;

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
    pub dismiss_id: Option<&'static str>,
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

#[derive(Clone)]
pub(crate) struct ValidationContext {
    pub key: RowKey,
    pub status: Option<AssetStatus>,
    pub local_root: PathBuf,
    pub prod_root: PathBuf,
}

impl From<&Request> for ValidationContext {
    fn from(request: &Request) -> Self {
        Self {
            key: request.key.clone(),
            status: request.status.clone(),
            local_root: request.local_root.clone(),
            prod_root: request.prod_root.clone(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Check {
    _name: &'static str,
    weight: usize,
    run: fn(&ValidationContext) -> Vec<Finding>,
}

impl Check {
    pub(crate) fn weight(self) -> usize {
        self.weight
    }

    pub(crate) fn run(self, ctx: &ValidationContext) -> Vec<Finding> {
        (self.run)(ctx)
    }
}

pub(crate) fn all_checks() -> &'static [Check] {
    const CHECKS: &[Check] = &[
        Check {
            _name: "root-entries",
            weight: 1,
            run: root_entries::run,
        },
        Check {
            _name: "local-freshness",
            weight: 1,
            run: local_freshness::run,
        },
        Check {
            _name: "needs-review",
            weight: 1,
            run: needs_review::run,
        },
    ];
    CHECKS
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn validate_asset(
    asset_type: AssetType,
    slug: &str,
    status: Option<&AssetStatus>,
    local_root: &Path,
    prod_root: &Path,
) -> Vec<Finding> {
    let ctx = ValidationContext {
        key: RowKey {
            asset_type,
            slug: slug.to_string(),
        },
        status: status.cloned(),
        local_root: local_root.to_path_buf(),
        prod_root: prod_root.to_path_buf(),
    };
    all_checks()
        .iter()
        .flat_map(|check| check.run(&ctx))
        .collect()
}

pub(crate) fn send_finished(tx: &Sender<Msg>) {
    let _ = tx.send(Msg::Finished);
}

pub(crate) fn is_needs_review(status: Option<&AssetStatus>) -> bool {
    status
        .map(|status| status.name.eq_ignore_ascii_case("Needs review"))
        .unwrap_or(false)
}

pub(crate) fn is_harmless_root_file(name: &OsStr) -> bool {
    matches!(
        name.to_string_lossy().to_ascii_lowercase().as_str(),
        "thumbs.db" | "desktop.ini" | ".ds_store" | "ehthumbs.db"
    )
}

#[cfg(test)]
pub(crate) use dismissed::{load_dismissed_warning_keys_from, save_dismissed_warning_keys_to};

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

    #[test]
    fn colorchart_warning_is_marked_dismissable() {
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
            finding.text == "Missing /staging/colorchart.zip in Prod"
                && finding.dismiss_id == Some("missing-colorchart-zip")
        }));
    }

    #[test]
    fn dismissed_warning_file_round_trips() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("dismissed.json");
        let keys = std::collections::HashSet::from([
            "HDRIs/sunny_field:missing-colorchart-zip".to_string(),
            "Textures/forest_floor:other".to_string(),
        ]);

        save_dismissed_warning_keys_to(&path, &keys).unwrap();
        let loaded = load_dismissed_warning_keys_from(&path).unwrap();

        assert_eq!(loaded, keys);
    }

}
