use anyhow::Result;
use crate::notion::Asset;

/// Persist `assets` to `%APPDATA%\phase\cache\<name>.json`.
pub fn save(name: &str, assets: &[Asset]) -> Result<()> {
    let path = crate::config::cache_dir()?.join(format!("{name}.json"));
    let data = serde_json::to_vec(assets)?;
    std::fs::write(&path, data)?;
    Ok(())
}

/// Load cached assets from disk, returning `None` if the cache is absent or corrupt.
pub fn load(name: &str) -> Option<Vec<Asset>> {
    let path = crate::config::cache_dir().ok()?.join(format!("{name}.json"));
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}
