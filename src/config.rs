use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub notion_token: String,
    #[serde(default = "default_prod_root")]
    pub prod_root: PathBuf,
    #[serde(default = "default_local_root")]
    pub local_root: PathBuf,
    /// Last selected asset-type tab ("HDRIs" or "Textures").
    #[serde(default)]
    pub last_tab: String,
    /// Last selected asset-type labels. Replaces `last_tab` while keeping it for migration.
    #[serde(default)]
    pub last_asset_types: Vec<String>,
    /// Last selected author filter, shared across asset-type selections.
    #[serde(default)]
    pub last_author_filter: String,
    /// Last author filter per asset-type label.
    #[serde(default)]
    pub last_filters: std::collections::HashMap<String, String>,
    #[serde(default = "default_skip_pull_raw_tif_if_many_work_tifs")]
    pub skip_pull_raw_tif_if_many_work_tifs: bool,
}

fn default_prod_root() -> PathBuf {
    PathBuf::from(r"P:\Assets")
}
fn default_local_root() -> PathBuf {
    PathBuf::from(r"C:\PHASE")
}
fn default_skip_pull_raw_tif_if_many_work_tifs() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            notion_token: String::new(),
            prod_root: default_prod_root(),
            local_root: default_local_root(),
            last_tab: String::new(),
            last_asset_types: Vec::new(),
            last_author_filter: String::new(),
            last_filters: std::collections::HashMap::new(),
            skip_pull_raw_tif_if_many_work_tifs: default_skip_pull_raw_tif_if_many_work_tifs(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn skip_pull_raw_tif_if_many_work_tifs_defaults_to_enabled() {
        assert!(super::Config::default().skip_pull_raw_tif_if_many_work_tifs);
    }

    #[test]
    fn missing_skip_pull_raw_tif_if_many_work_tifs_config_field_defaults_to_enabled() {
        let cfg: super::Config = toml::from_str(
            r#"
notion_token = ""
local_root = "C:\\PHASE"
"#,
        )
        .unwrap();

        assert!(cfg.skip_pull_raw_tif_if_many_work_tifs);
    }
}

/// Returns `%APPDATA%\phase`, creating it if missing.
pub fn app_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not locate %APPDATA%")?;
    let dir = base.join("phase");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(app_dir()?.join("config.toml"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(app_dir()?.join("phase.log"))
}

/// Returns `%APPDATA%\phase\cache`, creating it if missing.
pub fn cache_dir() -> Result<PathBuf> {
    let dir = app_dir()?.join("cache");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let text = toml::to_string_pretty(cfg).context("serialising config")?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
