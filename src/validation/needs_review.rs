use crate::ui::AssetType;
use crate::validation::{
    is_complete_status, is_needs_review, status_has_passed_review, Finding, Severity,
    ValidationContext,
};

pub(crate) fn run(ctx: &ValidationContext) -> Vec<Finding> {
    let is_active = is_needs_review(ctx.status.as_ref())
        || status_has_passed_review(ctx.status.as_ref(), &ctx.status_options);
    if !is_active || is_complete_status(ctx.status.as_ref()) || !ctx.prod_root.is_dir() {
        return Vec::new();
    }

    let staging = ctx.prod_root.join("staging");
    let slug = &ctx.key.slug;
    let mut findings = Vec::new();
    match ctx.key.asset_type {
        AssetType::Hdris => {
            if !staging.join(format!("{slug}.exr")).is_file() {
                findings.push(Finding {
                    severity: Severity::Error,
                    text: format!("Missing /staging/{slug}.exr in Prod"),
                    dismiss_id: None,
                });
            }
            if !staging.join("colorchart.zip").is_file() {
                findings.push(Finding {
                    severity: Severity::Warning,
                    text: "Missing /staging/colorchart.zip in Prod".into(),
                    dismiss_id: Some("missing-colorchart-zip"),
                });
            }
        }
        AssetType::Textures => {
            if !staging.join(format!("{slug}.blend")).is_file() {
                findings.push(Finding {
                    severity: Severity::Error,
                    text: format!("Missing /staging/{slug}.blend in Prod"),
                    dismiss_id: None,
                });
            }
            if !staging.join("textures").is_dir() {
                findings.push(Finding {
                    severity: Severity::Error,
                    text: "Missing /staging/textures in Prod".into(),
                    dismiss_id: None,
                });
            }
        }
    }
    findings
}
