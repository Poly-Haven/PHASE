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

/// One release's notes, as shown in the "What's new" dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub version: String,
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    /// A minor or major bump, which the status bar mentions once installed.
    /// Patches are installed just as eagerly, but silently.
    pub minor_or_major_update: bool,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The newest release ahead of the running version, if there is one.
pub fn check_for_update() -> Result<Option<UpdateInfo>> {
    let current = parse_version(current_version())?;
    // Releases are sorted oldest first, so the last one ahead of us is the newest.
    let latest = fetch_releases()?
        .into_iter()
        .rfind(|release| release.version > current);

    let Some(latest) = latest else {
        return Ok(None);
    };

    Ok(Some(UpdateInfo {
        version: latest.version.to_string(),
        tag: format!("v{}", latest.version),
        minor_or_major_update: minor_or_major_ahead(&current, &latest.version),
    }))
}

/// Notes for every release the user has not been shown yet: those newer than
/// `previous` and no newer than what is running, newest first. With no
/// `previous` recorded that is just the running version's own notes.
pub fn changelog_since(previous: Option<&str>) -> Result<Vec<ReleaseNotes>> {
    let current = parse_version(current_version())?;
    let previous = previous.and_then(|version| parse_version(version).ok());
    Ok(changelog_range(
        &current,
        previous.as_ref(),
        fetch_releases()?,
    ))
}

/// Whether the running version's notes are still owed to the user.
pub fn changelog_is_due(last_run: Option<&str>) -> bool {
    let Ok(current) = parse_version(current_version()) else {
        return false;
    };
    changelog_due(&current, last_run)
}

/// Download the newest binary and replace the running executable in place.
/// The running process keeps its own copy, so nothing changes until PHASE is
/// started again.
pub fn install_update(tag: &str) -> Result<()> {
    self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(TARGET)
        .target_version_tag(tag)
        .current_version(current_version())
        .show_download_progress(false)
        .show_output(false)
        .no_confirm(true)
        .build()
        .context("configure GitHub updater")?
        .update()
        .context("download and install update")?;
    Ok(())
}

/// Start a fresh PHASE process. Called while shutting down, so the new process
/// picks up whatever `install_update` left on disk.
pub fn relaunch() -> Result<()> {
    let exe = std::env::current_exe().context("locate current executable")?;
    std::process::Command::new(exe)
        .spawn()
        .context("relaunch PHASE")?;
    Ok(())
}

fn fetch_releases() -> Result<Vec<ReleaseInfo>> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .with_target(TARGET)
        .build()
        .context("configure GitHub release lookup")?
        .fetch()
        .context("fetch GitHub releases")?;

    let mut parsed = releases
        .into_iter()
        .filter_map(|release| {
            Some(ReleaseInfo {
                version: parse_version(&release.version).ok()?,
                notes: release.body,
            })
        })
        .collect::<Vec<_>>();
    parsed.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(parsed)
}

fn parse_version(version: &str) -> Result<Version> {
    Version::parse(version.trim_start_matches('v'))
        .map_err(|err| anyhow!("invalid version {version}: {err}"))
}

fn minor_or_major_ahead(current: &Version, latest: &Version) -> bool {
    latest.major > current.major || (latest.major == current.major && latest.minor > current.minor)
}

fn changelog_due(current: &Version, last_run: Option<&str>) -> bool {
    match last_run.map(parse_version) {
        // Nothing recorded, or a version we cannot read: introduce this one.
        None | Some(Err(_)) => true,
        Some(Ok(previous)) => previous < *current,
    }
}

fn changelog_range(
    current: &Version,
    previous: Option<&Version>,
    releases: Vec<ReleaseInfo>,
) -> Vec<ReleaseNotes> {
    let mut entries = releases
        .into_iter()
        .filter(|release| match previous {
            None => release.version == *current,
            Some(previous) => release.version > *previous && release.version <= *current,
        })
        .map(|release| ReleaseNotes {
            version: release.version.to_string(),
            notes: release.notes.unwrap_or_default().trim().to_string(),
        })
        .collect::<Vec<_>>();
    // `fetch_releases` sorts oldest first; the newest release leads the dialog.
    entries.reverse();
    entries
}

#[cfg(test)]
mod tests {
    use super::{changelog_due, changelog_range, minor_or_major_ahead, ReleaseInfo};
    use semver::Version;

    fn version(version: &str) -> Version {
        Version::parse(version).unwrap()
    }

    fn release(v: &str, notes: Option<&str>) -> ReleaseInfo {
        ReleaseInfo {
            version: version(v),
            notes: notes.map(|notes| notes.to_string()),
        }
    }

    #[test]
    fn patch_update_is_not_minor_or_major_ahead() {
        assert!(!minor_or_major_ahead(&version("0.1.0"), &version("0.1.1")));
    }

    #[test]
    fn minor_update_is_minor_or_major_ahead() {
        assert!(minor_or_major_ahead(&version("0.1.9"), &version("0.2.0")));
    }

    #[test]
    fn major_update_is_minor_or_major_ahead() {
        assert!(minor_or_major_ahead(&version("0.9.9"), &version("1.0.0")));
    }

    #[test]
    fn the_changelog_is_due_for_a_version_never_run_before() {
        let current = version("1.7.0");

        assert!(changelog_due(&current, None));
        assert!(changelog_due(&current, Some("1.6.0")));
        assert!(changelog_due(&current, Some("not a version")));
    }

    #[test]
    fn the_changelog_is_not_due_again_for_the_same_version() {
        let current = version("1.7.0");

        assert!(!changelog_due(&current, Some("1.7.0")));
        assert!(
            !changelog_due(&current, Some("1.8.0")),
            "no downgrade notes"
        );
    }

    #[test]
    fn the_changelog_covers_every_version_between_the_last_run_and_this_one() {
        let releases = vec![
            release("1.5.0", Some("Five")),
            release("1.6.0", Some("Six")),
            release("1.7.0", Some("Seven")),
            release("1.8.0", Some("Eight — not installed yet")),
        ];

        let entries = changelog_range(&version("1.7.0"), Some(&version("1.5.0")), releases);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.7.0", "1.6.0"],
            "newest first, and never past the version actually running"
        );
        assert_eq!(entries[0].notes, "Seven");
    }

    #[test]
    fn the_first_ever_changelog_covers_only_the_running_version() {
        let releases = vec![
            release("1.6.0", Some("Six")),
            release("1.7.0", Some("Seven")),
        ];

        let entries = changelog_range(&version("1.7.0"), None, releases);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, "1.7.0");
    }

    #[test]
    fn a_release_without_notes_still_gets_an_entry() {
        let entries = changelog_range(
            &version("1.7.0"),
            Some(&version("1.6.0")),
            vec![release("1.7.0", None)],
        );

        assert_eq!(entries.len(), 1);
        assert!(entries[0].notes.is_empty());
    }
}
