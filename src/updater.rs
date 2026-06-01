use anyhow::{anyhow, Context, Result};
use semver::Version;

const REPO_OWNER: &str = "Poly-Haven";
const REPO_NAME: &str = "PHASE";
const BIN_NAME: &str = "phase";
const TARGET: &str = "x86_64-pc-windows-msvc";

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

    let mut newer = releases
        .into_iter()
        .filter_map(|release| {
            let version = parse_version(&release.version).ok()?;
            (version > current).then_some((version, release))
        })
        .collect::<Vec<_>>();
    newer.sort_by(|(a, _), (b, _)| b.cmp(a));

    let Some((version, release)) = newer.into_iter().next() else {
        return Ok(None);
    };

    Ok(Some(UpdateInfo {
        version: version.to_string(),
        tag: format!("v{version}"),
        notes: release
            .body
            .unwrap_or_else(|| "No release notes provided.".into()),
        minor_or_major_update: is_minor_or_major_ahead(&current.to_string(), &version.to_string())?,
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

#[cfg(test)]
mod tests {
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
}
