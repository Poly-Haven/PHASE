use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

pub const HDRIS_DB_ID: &str = "21f373ac-61c1-80d0-8e55-cd46d121d1d5";
pub const TEXTURES_DB_ID: &str = "215373ac-61c1-80dd-8a97-edb25bb6a5f8";
const NOTION_VERSION: &str = "2022-06-28";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetList {
    pub assets: Vec<Asset>,
    pub statuses: Vec<StatusOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub page_id: String,
    pub slug: String,
    pub author: String,
    pub url: String,
    pub status: Option<AssetStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetStatus {
    pub id: String,
    pub name: String,
    pub color: String,
    pub group: StatusGroup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusOption {
    pub id: String,
    pub name: String,
    pub color: String,
    pub group: StatusGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusGroup {
    ToDo,
    InProgress,
    Complete,
}

impl StatusGroup {
    pub fn all() -> &'static [Self] {
        &[Self::ToDo, Self::InProgress, Self::Complete]
    }

    pub fn default_filter() -> Vec<Self> {
        vec![Self::InProgress]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ToDo => "To-do",
            Self::InProgress => "In progress",
            Self::Complete => "Complete",
        }
    }

    pub fn order(self) -> usize {
        match self {
            Self::ToDo => 0,
            Self::InProgress => 1,
            Self::Complete => 2,
        }
    }

    fn from_notion_group(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "to-do" | "to do" | "todo" | "not started" => Some(Self::ToDo),
            "in progress" | "doing" => Some(Self::InProgress),
            "complete" | "completed" | "done" => Some(Self::Complete),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct DatabaseResponse {
    properties: serde_json::Value,
}

#[derive(Deserialize)]
struct QueryResponse {
    results: Vec<Page>,
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Deserialize)]
struct Page {
    id: String,
    url: String,
    properties: serde_json::Value,
}

/// Fetch every page in a database, paginating until exhausted. Sorted by slug.
pub fn fetch_database(token: &str, database_id: &str) -> Result<AssetList> {
    let client = client()?;
    let statuses = fetch_status_options_with_client(&client, token, database_id)?;
    let status_by_id: HashMap<String, StatusOption> = statuses
        .iter()
        .map(|status| (status.id.clone(), status.clone()))
        .collect();
    let url = format!("https://api.notion.com/v1/databases/{database_id}/query");
    let mut cursor: Option<String> = None;
    let mut assets = Vec::new();

    loop {
        let mut body = serde_json::Map::new();
        body.insert("page_size".into(), serde_json::Value::from(100));
        if let Some(c) = &cursor {
            body.insert("start_cursor".into(), serde_json::Value::String(c.clone()));
        }

        let resp = client
            .post(&url)
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
            assets.push(Asset {
                page_id: p.id,
                slug: extract_title(&p.properties),
                author: extract_author(&p.properties),
                url: p.url,
                status: extract_status(&p.properties, &status_by_id),
            });
        }

        if page.has_more {
            cursor = page.next_cursor;
        } else {
            break;
        }
    }

    assets.sort_by(|a, b| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()));
    Ok(AssetList { assets, statuses })
}

pub fn update_page_status(token: &str, page_id: &str, status: &StatusOption) -> Result<()> {
    let client = client()?;
    let url = format!("https://api.notion.com/v1/pages/{page_id}");
    let body = serde_json::json!({
        "properties": {
            "Status": {
                "status": {
                    "name": status.name
                }
            }
        }
    });

    let resp = client
        .patch(url)
        .bearer_auth(token)
        .header("Notion-Version", NOTION_VERSION)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .context("HTTP request to update Notion status")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("Notion API error {status}: {text}"));
    }

    Ok(())
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")
}

fn fetch_status_options_with_client(
    client: &reqwest::blocking::Client,
    token: &str,
    database_id: &str,
) -> Result<Vec<StatusOption>> {
    let url = format!("https://api.notion.com/v1/databases/{database_id}");
    let resp = client
        .get(url)
        .bearer_auth(token)
        .header("Notion-Version", NOTION_VERSION)
        .send()
        .context("HTTP request to Notion database metadata")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("Notion API error {status}: {text}"));
    }

    let db: DatabaseResponse = resp.json().context("parsing Notion database metadata")?;
    Ok(extract_status_options(&db.properties))
}

fn extract_status_options(props: &serde_json::Value) -> Vec<StatusOption> {
    let Some(status_prop) = props.get("Status") else {
        return Vec::new();
    };
    let Some(status) = status_prop.get("status") else {
        return Vec::new();
    };

    let mut groups_by_id: HashMap<String, StatusGroup> = HashMap::new();
    let mut groups_by_option_id: HashMap<String, StatusGroup> = HashMap::new();
    for group in status
        .get("groups")
        .and_then(|g| g.as_array())
        .into_iter()
        .flatten()
    {
        let Some(group_name) = group.get("name").and_then(|name| name.as_str()) else {
            continue;
        };
        let Some(status_group) = StatusGroup::from_notion_group(group_name) else {
            continue;
        };
        if let Some(group_id) = group.get("id").and_then(|id| id.as_str()) {
            groups_by_id.insert(group_id.to_string(), status_group);
        }
        if let Some(option_ids) = group.get("option_ids").and_then(|ids| ids.as_array()) {
            for option_id in option_ids.iter().filter_map(|id| id.as_str()) {
                groups_by_option_id.insert(option_id.to_string(), status_group);
            }
        }
    }

    let mut options: Vec<_> = status
        .get("options")
        .and_then(|o| o.as_array())
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let id = option.get("id")?.as_str()?.to_string();
            let name = option.get("name")?.as_str()?.to_string();
            let color = option
                .get("color")
                .and_then(|c| c.as_str())
                .unwrap_or("default")
                .to_string();
            let group = option
                .get("group_id")
                .and_then(|g| g.as_str())
                .and_then(|id| groups_by_id.get(id))
                .or_else(|| groups_by_option_id.get(&id))
                .copied()?;
            Some(StatusOption {
                id,
                name,
                color,
                group,
            })
        })
        .collect();
    options.sort_by(|a, b| {
        a.group
            .order()
            .cmp(&b.group.order())
            .then_with(|| a.name.cmp(&b.name))
    });
    options
}

fn extract_status(
    props: &serde_json::Value,
    status_by_id: &HashMap<String, StatusOption>,
) -> Option<AssetStatus> {
    let status = props.get("Status")?.get("status")?;
    let id = status.get("id")?.as_str()?.to_string();
    let fallback_name = status
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let fallback_color = status
        .get("color")
        .and_then(|c| c.as_str())
        .unwrap_or("default")
        .to_string();
    if let Some(option) = status_by_id.get(&id) {
        return Some(AssetStatus {
            id: option.id.clone(),
            name: option.name.clone(),
            color: option.color.clone(),
            group: option.group,
        });
    }
    Some(AssetStatus {
        id,
        name: fallback_name,
        color: fallback_color,
        group: StatusGroup::InProgress,
    })
}

fn extract_title(props: &serde_json::Value) -> String {
    let Some(obj) = props.as_object() else {
        return String::new();
    };
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
    let Some(author) = props.get("Author") else {
        return String::new();
    };
    let ty = author.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "rich_text" => author
            .get("rich_text")
            .and_then(|a| a.as_array())
            .map(|a| concat_plain_text(a))
            .unwrap_or_default(),
        "title" => author
            .get("title")
            .and_then(|a| a.as_array())
            .map(|a| concat_plain_text(a))
            .unwrap_or_default(),
        "people" => author
            .get("people")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        "select" => author
            .get("select")
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str())
            .map(String::from)
            .unwrap_or_default(),
        "multi_select" => author
            .get("multi_select")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn concat_plain_text(arr: &[serde_json::Value]) -> String {
    arr.iter()
        .filter_map(|t| t.get("plain_text").and_then(|p| p.as_str()))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_status_options_with_group_and_color() {
        let props = serde_json::json!({
            "Status": {
                "type": "status",
                "status": {
                    "options": [
                        { "id": "a", "name": "Awaiting payment", "color": "yellow", "group_id": "g2" },
                        { "id": "b", "name": "Done", "color": "green", "group_id": "g3" }
                    ],
                    "groups": [
                        { "id": "g1", "name": "To-do" },
                        { "id": "g2", "name": "In progress" },
                        { "id": "g3", "name": "Complete" }
                    ]
                }
            }
        });

        let options = extract_status_options(&props);

        assert_eq!(
            options,
            vec![
                StatusOption {
                    id: "a".into(),
                    name: "Awaiting payment".into(),
                    color: "yellow".into(),
                    group: StatusGroup::InProgress,
                },
                StatusOption {
                    id: "b".into(),
                    name: "Done".into(),
                    color: "green".into(),
                    group: StatusGroup::Complete,
                },
            ]
        );
    }

    #[test]
    fn extracts_status_option_groups_from_group_option_ids() {
        let props = serde_json::json!({
            "Status": {
                "type": "status",
                "status": {
                    "options": [
                        { "id": "todo", "name": "To do", "color": "default" },
                        { "id": "review", "name": "Creative review", "color": "blue" },
                        { "id": "done", "name": "Done", "color": "green" }
                    ],
                    "groups": [
                        { "id": "g1", "name": "To-do", "option_ids": ["todo"] },
                        { "id": "g2", "name": "In progress", "option_ids": ["review"] },
                        { "id": "g3", "name": "Complete", "option_ids": ["done"] }
                    ]
                }
            }
        });

        let options = extract_status_options(&props);

        assert_eq!(
            options.iter().map(|o| (&o.id, o.group)).collect::<Vec<_>>(),
            vec![
                (&"todo".to_string(), StatusGroup::ToDo),
                (&"review".to_string(), StatusGroup::InProgress),
                (&"done".to_string(), StatusGroup::Complete),
            ]
        );
    }

    #[test]
    fn extracts_page_status_group_from_database_option() {
        let props = serde_json::json!({
            "Status": {
                "type": "status",
                "status": { "id": "a", "name": "Awaiting payment", "color": "yellow" }
            }
        });
        let status_by_id = HashMap::from([(
            "a".to_string(),
            StatusOption {
                id: "a".into(),
                name: "Awaiting payment".into(),
                color: "yellow".into(),
                group: StatusGroup::InProgress,
            },
        )]);

        assert_eq!(
            extract_status(&props, &status_by_id),
            Some(AssetStatus {
                id: "a".into(),
                name: "Awaiting payment".into(),
                color: "yellow".into(),
                group: StatusGroup::InProgress,
            })
        );
    }
}
