use walkdir::WalkDir;

use crate::validation::{
    is_complete_status, is_harmless_root_file, is_needs_review, Finding, Severity, ValidationContext,
};

pub(crate) fn run(ctx: &ValidationContext) -> Vec<Finding> {
    if is_complete_status(ctx.status.as_ref())
        || !ctx.local_root.is_dir()
        || !ctx.prod_root.is_dir()
    {
        return Vec::new();
    }

    if !local_is_newer_or_extra(ctx) || !is_needs_review(ctx.status.as_ref()) {
        return Vec::new();
    }

    vec![Finding {
        severity: Severity::Warning,
        text: "Local files newer than Prod. Push?".into(),
        dismiss_id: None,
    }]
}

fn local_is_newer_or_extra(ctx: &ValidationContext) -> bool {
    for entry in WalkDir::new(&ctx.local_root)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(&ctx.local_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        if is_harmless_root_file(entry.file_name()) {
            continue;
        }
        let prod_path = ctx.prod_root.join(rel);
        if !prod_path.is_file() {
            return true;
        }
        let local_mtime = match std::fs::metadata(entry.path()).and_then(|meta| meta.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };
        let prod_mtime = match std::fs::metadata(&prod_path).and_then(|meta| meta.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };
        if local_mtime > prod_mtime {
            return true;
        }
    }
    false
}
