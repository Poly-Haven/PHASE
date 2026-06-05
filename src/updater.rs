use anyhow::{anyhow, Context, Result};
use semver::Version;

const REPO_OWNER: &str = "Poly-Haven";
const REPO_NAME: &str = "PHASE";
const BIN_NAME: &str = "phase";
const TARGET: &str = "x86_64-pc-windows-msvc";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReleaseInfo {
    version: Version,
    notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub minor_or_major_update: bool,
}

pub fn check_for_update() -> Result<Option<UpdateInfo>> {
    let current = parse_version(env!("CARGO_PKG_VERSION"))?;
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .with_target(TARGET)
        .build()
        .context("configure GitHub release lookup")?
        .fetch()
        .context("fetch GitHub releases")?;

    let newer = releases
        .into_iter()
        .filter_map(|release| {
            let version = parse_version(&release.version).ok()?;
            (version > current).then_some(ReleaseInfo {
                version,
                notes: release.body,
            })
        })
        .collect::<Vec<_>>();
    let newer = newer_releases(&current, newer);

    let Some(latest) = newer.last() else {
        return Ok(None);
    };

    Ok(Some(UpdateInfo {
        version: latest.version.to_string(),
        tag: format!("v{}", latest.version),
        notes: format_release_notes(&current, &newer),
        minor_or_major_update: is_minor_or_major_ahead(
            &current.to_string(),
            &latest.version.to_string(),
        )?,
    }))
}

pub fn install_update_and_restart(tag: &str) -> Result<()> {
    self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(TARGET)
        .target_version_tag(tag)
        .current_version(env!("CARGO_PKG_VERSION"))
        .show_download_progress(false)
        .show_output(false)
        .no_confirm(true)
        .build()
        .context("configure GitHub updater")?
        .update()
        .context("download and install update")?;

    let exe = std::env::current_exe().context("locate current executable")?;
    std::process::Command::new(exe)
        .spawn()
        .context("restart PHASE after update")?;
    std::process::exit(0);
}

pub fn is_minor_or_major_ahead(current: &str, latest: &str) -> Result<bool> {
    let current = parse_version(current)?;
    let latest = parse_version(latest)?;
    Ok(latest.major > current.major
        || (latest.major == current.major && latest.minor > current.minor))
}

fn parse_version(version: &str) -> Result<Version> {
    Version::parse(version.trim_start_matches('v'))
        .map_err(|err| anyhow!("invalid version {version}: {err}"))
}

fn newer_releases(current: &Version, mut releases: Vec<ReleaseInfo>) -> Vec<ReleaseInfo> {
    releases.retain(|release| release.version > *current);
    releases.sort_by(|a, b| a.version.cmp(&b.version));
    releases
}

fn format_release_notes(current: &Version, releases: &[ReleaseInfo]) -> String {
    releases
        .iter()
        .filter(|release| release.version > *current)
        .map(|release| {
            let body = release
                .notes
                .clone()
                .unwrap_or_else(|| "No release notes provided.".into());
            format!("## v{}\n\n{}", release.version, body)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::{format_release_notes, newer_releases, ReleaseInfo};
    use semver::Version;

    fn release(version: &str, notes: Option<&str>) -> ReleaseInfo {
        ReleaseInfo {
            version: Version::parse(version).unwrap(),
            notes: notes.map(|notes| notes.to_string()),
        }
    }

    #[test]
    fn patch_update_is_not_minor_or_major_ahead() {
        assert!(!super::is_minor_or_major_ahead("0.1.0", "0.1.1").unwrap());
    }

    #[test]
    fn minor_update_is_minor_or_major_ahead() {
        assert!(super::is_minor_or_major_ahead("0.1.9", "0.2.0").unwrap());
    }

    #[test]
    fn major_update_is_minor_or_major_ahead() {
        assert!(super::is_minor_or_major_ahead("0.9.9", "1.0.0").unwrap());
    }

    #[test]
    fn newer_releases_keeps_only_versions_ahead_of_current_and_sorts_them() {
        let current = Version::parse("1.2.1").unwrap();
        let releases = vec![
            release("1.2.3", Some("Three")),
            release("1.2.2", Some("Two")),
            release("1.2.1", Some("Current")),
        ];

        let newer = newer_releases(&current, releases);

        assert_eq!(
            newer
                .iter()
                .map(|release| release.version.to_string())
                .collect::<Vec<_>>(),
            vec!["1.2.2", "1.2.3"]
        );
    }

    #[test]
    fn release_notes_include_every_newer_version() {
        let current = Version::parse("1.2.1").unwrap();
        let releases = vec![
            release("1.2.3", Some("Notes for 1.2.3")),
            release("1.2.2", Some("Notes for 1.2.2")),
        ];

        let notes = format_release_notes(&current, &releases);

        assert!(notes.contains("## v1.2.2"));
        assert!(notes.contains("Notes for 1.2.2"));
        assert!(notes.contains("## v1.2.3"));
        assert!(notes.contains("Notes for 1.2.3"));
    }
}
