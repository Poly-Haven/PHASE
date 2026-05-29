use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublishedAssets {
    pub slugs: HashSet<String>,
}

pub fn cache_name() -> &'static str {
    "polyhaven_published"
}

pub fn fetch_published_assets() -> Result<PublishedAssets> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;
    let value: serde_json::Value = client
        .get("https://api.polyhaven.com/assets?t=all&future=true")
        .send()
        .context("HTTP request to Poly Haven assets API")?
        .error_for_status()
        .context("Poly Haven assets API status")?
        .json()
        .context("parsing Poly Haven assets response")?;
    Ok(PublishedAssets {
        slugs: parse_slugs(&value),
    })
}

pub fn parse_slugs(value: &serde_json::Value) -> HashSet<String> {
    match value {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("slug").and_then(|slug| slug.as_str()))
            .map(str::to_string)
            .collect(),
        _ => HashSet::new(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_published_asset_slugs_from_api_object_keys() {
        let value = serde_json::json!({
            "a_slug": { "type": "hdri" },
            "another_slug": { "type": "texture" }
        });

        assert!(super::parse_slugs(&value).contains("a_slug"));
        assert!(super::parse_slugs(&value).contains("another_slug"));
    }
}
