use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const HDRIS_DB_ID:    &str = "21f373ac-61c1-80d0-8e55-cd46d121d1d5";
pub const TEXTURES_DB_ID: &str = "215373ac-61c1-80dd-8a97-edb25bb6a5f8";
const NOTION_VERSION: &str = "2022-06-28";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub slug:   String,
    pub author: String,
    pub url:    String,
}

#[derive(Deserialize)]
struct QueryResponse {
    results:     Vec<Page>,
    next_cursor: Option<String>,
    has_more:    bool,
}

#[derive(Deserialize)]
struct Page {
    url:        String,
    properties: serde_json::Value,
}

/// Fetch every page in a database, paginating until exhausted. Sorted by slug.
pub fn fetch_database(token: &str, database_id: &str) -> Result<Vec<Asset>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;

    let url = format!("https://api.notion.com/v1/databases/{database_id}/query");
    let mut cursor: Option<String> = None;
    let mut out = Vec::new();

    loop {
        let mut body = serde_json::Map::new();
        body.insert("page_size".into(), serde_json::Value::from(100));
        if let Some(c) = &cursor {
            body.insert("start_cursor".into(), serde_json::Value::String(c.clone()));
        }

        let resp = client.post(&url)
            .bearer_auth(token)
            .header("Notion-Version", NOTION_VERSION)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .context("HTTP request to Notion")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(anyhow!("Notion API error {status}: {text}"));
        }

        let page: QueryResponse = resp.json().context("parsing Notion response")?;
        for p in page.results {
            out.push(Asset {
                slug:   extract_title(&p.properties),
                author: extract_author(&p.properties),
                url:    p.url,
            });
        }

        if page.has_more { cursor = page.next_cursor; } else { break; }
    }

    out.sort_by(|a, b| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()));
    Ok(out)
}

fn extract_title(props: &serde_json::Value) -> String {
    let Some(obj) = props.as_object() else { return String::new(); };
    for (_name, val) in obj {
        if val.get("type").and_then(|t| t.as_str()) == Some("title") {
            if let Some(arr) = val.get("title").and_then(|t| t.as_array()) {
                return concat_plain_text(arr);
            }
        }
    }
    String::new()
}

/// `Author` may be `people`, `rich_text`, `title`, `select`, or `multi_select`.
fn extract_author(props: &serde_json::Value) -> String {
    let Some(author) = props.get("Author") else { return String::new(); };
    let ty = author.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "rich_text" => author.get("rich_text").and_then(|a| a.as_array())
            .map(|a| concat_plain_text(a)).unwrap_or_default(),
        "title" => author.get("title").and_then(|a| a.as_array())
            .map(|a| concat_plain_text(a)).unwrap_or_default(),
        "people" => author.get("people").and_then(|a| a.as_array())
            .map(|arr| arr.iter()
                 .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
                 .collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
        "select" => author.get("select").and_then(|s| s.get("name"))
            .and_then(|n| n.as_str()).map(String::from).unwrap_or_default(),
        "multi_select" => author.get("multi_select").and_then(|a| a.as_array())
            .map(|arr| arr.iter()
                 .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                 .collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn concat_plain_text(arr: &[serde_json::Value]) -> String {
    arr.iter()
        .filter_map(|t| t.get("plain_text").and_then(|p| p.as_str()))
        .collect::<String>()
}
