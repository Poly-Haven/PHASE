use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};

/// Persist `assets` to `%APPDATA%\phase\cache\<name>.json`.
pub fn save<T: Serialize>(name: &str, assets: &T) -> Result<()> {
    let path = crate::config::cache_dir()?.join(format!("{name}.json"));
    let data = serde_json::to_vec(assets)?;
    std::fs::write(&path, data)?;
    Ok(())
}

/// Load cached assets from disk, returning `None` if the cache is absent or corrupt.
pub fn load<T: DeserializeOwned>(name: &str) -> Option<T> {
    let path = crate::config::cache_dir()
        .ok()?
        .join(format!("{name}.json"));
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}
