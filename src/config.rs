use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub auth_access_token: String,
    #[serde(default)]
    pub auth_refresh_token: String,
    #[serde(default)]
    pub auth_expires_at: Option<u64>,
    #[serde(default = "default_open_notion_links_in_desktop_app")]
    pub open_notion_links_in_desktop_app: bool,
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
    /// Last selected author filters, shared across asset-type selections.
    #[serde(default)]
    pub last_author_filters: Vec<String>,
    /// Last selected author filters per asset-type label.
    #[serde(default)]
    pub last_author_filters_by_type: std::collections::HashMap<String, Vec<String>>,
    /// Last author filter per asset-type label.
    #[serde(default)]
    pub last_filters: std::collections::HashMap<String, String>,
    #[serde(default = "default_skip_pull_raw_tif_if_many_work_tifs")]
    pub skip_pull_raw_tif_if_many_work_tifs: bool,
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
    #[serde(default)]
    pub window_pos: Option<[f32; 2]>,
    /// Last selected status group filters (e.g. ["InProgress", "ToDo"]).
    #[serde(default)]
    pub last_selected_status_groups: Vec<crate::notion::StatusGroup>,
    /// UTC day number (`unix_seconds / 86400`) when update checks last ran.
    #[serde(default)]
    pub last_update_check_day: Option<u64>,
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
fn default_open_notion_links_in_desktop_app() -> bool {
    false
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auth_access_token: String::new(),
            auth_refresh_token: String::new(),
            auth_expires_at: None,
            open_notion_links_in_desktop_app: default_open_notion_links_in_desktop_app(),
            prod_root: default_prod_root(),
            local_root: default_local_root(),
            last_tab: String::new(),
            last_asset_types: Vec::new(),
            last_author_filter: String::new(),
            last_author_filters: Vec::new(),
            last_author_filters_by_type: std::collections::HashMap::new(),
            last_filters: std::collections::HashMap::new(),
            skip_pull_raw_tif_if_many_work_tifs: default_skip_pull_raw_tif_if_many_work_tifs(),
            last_selected_status_groups: Vec::new(),
            window_size: None,
            window_pos: None,
            last_update_check_day: None,
        }
    }
}

impl Config {
    pub fn access_token_expired_at(&self, now_unix_seconds: u64) -> bool {
        match self.auth_expires_at {
            Some(expires_at) => expires_at <= now_unix_seconds + 60,
            None => true,
        }
    }

    pub fn can_refresh_access_token(&self) -> bool {
        !self.auth_refresh_token.trim().is_empty()
    }

    pub fn has_access_token(&self) -> bool {
        !self.auth_access_token.trim().is_empty()
    }

}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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

    #[test]
    fn auth_tokens_default_to_empty_and_legacy_notion_token_is_ignored() {
        let cfg: super::Config = toml::from_str(
            r#"
notion_token = "legacy-secret-that-must-not-be-used"
auth_access_token = "access"
auth_refresh_token = "refresh"
auth_expires_at = 12345
local_root = "C:\\PHASE"
"#,
        )
        .unwrap();

        assert_eq!(cfg.auth_access_token, "access");
        assert_eq!(cfg.auth_refresh_token, "refresh");
        assert_eq!(cfg.auth_expires_at, Some(12345));
        assert!(!cfg.open_notion_links_in_desktop_app);
        assert_eq!(toml::to_string(&cfg).unwrap().contains("notion_token"), false);

        let default_cfg = super::Config::default();
        assert!(default_cfg.auth_access_token.is_empty());
        assert!(default_cfg.auth_refresh_token.is_empty());
        assert_eq!(default_cfg.auth_expires_at, None);
    }

    #[test]
    fn expired_access_token_requires_refresh_before_api_calls() {
        let mut cfg = super::Config::default();
        cfg.auth_access_token = "access".into();
        cfg.auth_refresh_token = "refresh".into();
        cfg.auth_expires_at = Some(1_000);

        assert!(cfg.access_token_expired_at(940));
        assert!(cfg.access_token_expired_at(1_000));
        assert!(!cfg.access_token_expired_at(939));
        assert!(cfg.can_refresh_access_token());
    }

    #[test]
    fn load_recovers_from_a_corrupted_primary_config_using_the_backup() {
        let temp = tempfile::tempdir().unwrap();

        let mut cfg = super::Config::default();
        cfg.auth_access_token = "access".into();
        cfg.auth_refresh_token = "refresh".into();
        cfg.auth_expires_at = Some(12345);
        cfg.local_root = PathBuf::from(r"C:\PHASE");
        let config_path = temp.path().join("config.toml");
        super::save_to_path(&config_path, &cfg).unwrap();

        let mut updated = cfg.clone();
        updated.local_root = PathBuf::from(r"C:\PHASE\changed");
        super::save_to_path(&config_path, &updated).unwrap();

        std::fs::write(&config_path, "this is not valid toml").unwrap();

        let loaded = super::load_from_path(&config_path).unwrap();
        assert_eq!(loaded.auth_access_token, "access");
        assert_eq!(loaded.auth_refresh_token, "refresh");
        assert_eq!(loaded.auth_expires_at, Some(12345));
        assert_eq!(loaded.local_root, PathBuf::from(r"C:\PHASE"));
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
    match load_from_path(&path) {
        Ok(cfg) => Ok(cfg),
        Err(primary_err) => {
            let backup = backup_path(&path);
            if backup.exists() {
                match load_from_path(&backup) {
                    Ok(cfg) => Ok(cfg),
                    Err(_) => Err(primary_err),
                }
            } else if !path.exists() {
                Ok(Config::default())
            } else {
                Err(primary_err)
            }
        }
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    save_to_path(&path, cfg)
}

fn load_from_path(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Err(anyhow::anyhow!("missing {}", path.display()));
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    match toml::from_str::<Config>(&text) {
        Ok(cfg) => Ok(cfg),
        Err(primary_err) => {
            let backup = backup_path(path);
            if !backup.exists() {
                return Err(primary_err).with_context(|| format!("parsing {}", path.display()));
            }
            let backup_text =
                fs::read_to_string(&backup).with_context(|| format!("reading {}", backup.display()))?;
            let backup_cfg: Config = toml::from_str(&backup_text)
                .with_context(|| format!("parsing {}", backup.display()))?;
            Ok(backup_cfg)
        }
    }
}

fn save_to_path(path: &Path, cfg: &Config) -> Result<()> {
    let text = toml::to_string_pretty(cfg).context("serialising config")?;
    save_text_atomically(path, &text)
}

fn save_text_atomically(path: &Path, text: &str) -> Result<()> {
    let temp = temp_path(path);
    let backup = backup_path(path);
    let _ = fs::remove_file(&temp);
    let _ = fs::remove_file(&backup);

    fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    if path.exists() {
        fs::rename(path, &backup)
            .with_context(|| format!("backing up {} -> {}", path.display(), backup.display()))?;
    }

    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            if backup.exists() {
                let _ = fs::rename(&backup, path);
            }
            let _ = fs::remove_file(&temp);
            Err(err).with_context(|| format!("replacing {}", path.display()))
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

fn temp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}
