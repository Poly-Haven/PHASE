use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ui::RowKey;

#[derive(Default, Serialize, Deserialize)]
struct DismissedWarningsFile {
    keys: Vec<String>,
}

pub fn dismissal_key(key: &RowKey, dismiss_id: &str) -> String {
    format!("{}/{}:{}", key.asset_type.folder(), key.slug, dismiss_id)
}

pub fn load_dismissed_warning_keys() -> Result<HashSet<String>> {
    load_dismissed_warning_keys_from(&dismissed_warning_path()?)
}

pub fn save_dismissed_warning_keys(keys: &HashSet<String>) -> Result<()> {
    save_dismissed_warning_keys_to(&dismissed_warning_path()?, keys)
}

fn dismissed_warning_path() -> Result<PathBuf> {
    Ok(crate::config::app_dir()?.join("dismissed_validation_warnings.json"))
}

pub(crate) fn load_dismissed_warning_keys_from(path: &Path) -> Result<HashSet<String>> {
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file: DismissedWarningsFile =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(file.keys.into_iter().collect())
}

pub(crate) fn save_dismissed_warning_keys_to(path: &Path, keys: &HashSet<String>) -> Result<()> {
    let mut sorted_keys = keys.iter().cloned().collect::<Vec<_>>();
    sorted_keys.sort();
    let text = serde_json::to_string_pretty(&DismissedWarningsFile { keys: sorted_keys })
        .context("serialising dismissed warnings")?;
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
