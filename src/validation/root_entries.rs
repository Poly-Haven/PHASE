use crate::ui::AssetType;
use crate::validation::{is_harmless_root_file, Finding, Severity, ValidationContext};

pub(crate) fn run(ctx: &ValidationContext) -> Vec<Finding> {
    if !ctx.local_root.is_dir() {
        return Vec::new();
    }

    let primary = match ctx.key.asset_type {
        AssetType::Hdris | AssetType::Textures => "raw",
    };
    let mut unexpected = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(&ctx.local_root) else {
        return Vec::new();
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !matches!(name.as_str(), "work" | "staging") && name != primary {
                unexpected.push(name);
            }
        } else if !is_harmless_root_file(entry.file_name().as_os_str()) {
            unexpected.push(name);
        }
    }

    if unexpected.is_empty() {
        Vec::new()
    } else {
        unexpected.sort_unstable();
        vec![Finding {
            severity: Severity::Error,
            text: format!("Unexpected root entries: {}", unexpected.join(", ")),
            dismiss_id: None,
        }]
    }
}
