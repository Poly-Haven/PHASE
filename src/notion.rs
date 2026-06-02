use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::HashMap;
use std::time::Duration;

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
    #[serde(default)]
    pub sort_order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusOption {
    pub id: String,
    pub name: String,
    pub color: String,
    pub group: StatusGroup,
    #[serde(default)]
    pub sort_order: usize,
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

    #[cfg(test)]
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
struct StatusUpdateResponse {
    #[allow(dead_code)]
    status: AssetStatus,
}

/// Fetch assets from the PHASE backend. Sorted by slug for stable UI display.
pub fn fetch_assets(token: &str, asset_type: &str) -> Result<AssetList> {
    let client = client()?;
    let url = format!(
        "{}api/phase/assets?type={asset_type}",
        crate::auth::phase_api_base_url()
    );
    let resp = client
        .get(url)
        .bearer_auth(token)
        .send()
        .context("HTTP request to PHASE asset API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("PHASE asset API error {status}: {text}"));
    }

    let mut list: AssetList = resp.json().context("parsing PHASE asset response")?;
    list.assets
        .sort_by(|a, b| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()));
    list.statuses.sort_by_key(|status| status.sort_order);
    Ok(list)
}

pub fn update_page_status(token: &str, page_id: &str, status: &StatusOption) -> Result<()> {
    let client = client()?;
    let url = format!(
        "{}api/phase/assets/{page_id}/status",
        crate::auth::phase_api_base_url()
    );
    let body = serde_json::json!({
        "status_id": status.id,
        "status_name": status.name,
    });

    let resp = client
        .patch(url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .context("HTTP request to update PHASE asset status")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("PHASE asset API error {status}: {text}"));
    }

    let _: StatusUpdateResponse = resp.json().context("parsing PHASE status response")?;
    Ok(())
}

pub fn rename_page_title(token: &str, page_id: &str, new_title: &str) -> Result<()> {
    let client = client()?;
    let url = format!(
        "{}api/phase/assets/{page_id}/title",
        crate::auth::phase_api_base_url()
    );
    let body = serde_json::json!({ "title": new_title });

    let resp = client
        .patch(url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .context("HTTP request to rename PHASE asset title")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("PHASE asset title API error {status}: {text}"));
    }

    Ok(())
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")
}

#[cfg(test)]
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

    status
        .get("options")
        .and_then(|o| o.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(sort_order, option)| {
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
                sort_order,
            })
        })
        .collect()
}

#[cfg(test)]
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
            sort_order: option.sort_order,
        });
    }
    Some(AssetStatus {
        id,
        name: fallback_name,
        color: fallback_color,
        group: StatusGroup::InProgress,
        sort_order: usize::MAX,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_phase_api_asset_list_with_status_color_and_sort_order() {
        let list: AssetList = serde_json::from_value(serde_json::json!({
            "assets": [
                {
                    "page_id": "page-1",
                    "slug": "asset_slug",
                    "author": "Author",
                    "url": "https://admin.polyhaven.com/assets/page-1",
                    "status": {
                        "id": "review",
                        "name": "Creative review",
                        "color": "blue",
                        "group": "InProgress",
                        "sort_order": 20
                    }
                }
            ],
            "statuses": [
                {
                    "id": "todo",
                    "name": "To-do",
                    "color": "gray",
                    "group": "ToDo",
                    "sort_order": 10
                },
                {
                    "id": "review",
                    "name": "Creative review",
                    "color": "blue",
                    "group": "InProgress",
                    "sort_order": 20
                }
            ]
        }))
        .unwrap();

        assert_eq!(list.assets[0].status.as_ref().unwrap().sort_order, 20);
        assert_eq!(list.statuses[0].sort_order, 10);
        assert_eq!(list.statuses[1].color, "blue");
    }

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
                    sort_order: 0,
                },
                StatusOption {
                    id: "b".into(),
                    name: "Done".into(),
                    color: "green".into(),
                    group: StatusGroup::Complete,
                    sort_order: 1,
                },
            ]
        );
    }

    #[test]
    fn preserves_status_options_in_notion_order() {
        let props = serde_json::json!({
            "Status": {
                "type": "status",
                "status": {
                    "options": [
                        { "id": "creative-review", "name": "Creative review", "color": "blue", "group_id": "g2" },
                        { "id": "awaiting-payment", "name": "Awaiting payment", "color": "yellow", "group_id": "g2" },
                        { "id": "todo", "name": "To do", "color": "default", "group_id": "g1" }
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
            options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>(),
            vec!["creative-review", "awaiting-payment", "todo"]
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
                sort_order: 0,
            },
        )]);

        assert_eq!(
            extract_status(&props, &status_by_id),
            Some(AssetStatus {
                id: "a".into(),
                name: "Awaiting payment".into(),
                color: "yellow".into(),
                group: StatusGroup::InProgress,
                sort_order: 0,
            })
        );
    }
}
