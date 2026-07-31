//! Measuring an asset's pixel resolution, and checking that a Texture/Model's
//! images agree on one.
//!
//! Reading image headers off the NAS is the expensive part, so two memos keep
//! it off the hot path:
//!
//! * `DIMENSIONS` caches per-file header reads, keyed by path + mtime + size,
//!   so repeat validation passes cost a `stat` instead of a network read (and
//!   the HDRI spec check reads the same header this module already read).
//! * `MEASURED` caches the per-asset result, so the worker pool's aggregating
//!   thread can attach a resolution to each row with a single map lookup rather
//!   than touching the disk again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::ui::{AssetType, RowKey};
use crate::validation::{Finding, Severity, ValidationContext};

/// Extensions we can read a header from (see the `image` crate features).
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "exr", "hdr"];

type Dimensions = (u32, u32);
/// A file identified by path plus the mtime and size it had when measured.
type FileRevision = (PathBuf, u64, u64);
type DimensionCache = Mutex<HashMap<FileRevision, Option<Dimensions>>>;

fn dimension_cache() -> &'static DimensionCache {
    static CACHE: OnceLock<DimensionCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn measured_cache() -> &'static Mutex<HashMap<RowKey, Option<Dimensions>>> {
    static CACHE: OnceLock<Mutex<HashMap<RowKey, Option<Dimensions>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read an image's dimensions from its header, memoised on (path, mtime, size).
///
/// Failures are cached too, so an unreadable file isn't retried on every pass.
pub(crate) fn dimensions(path: &Path) -> Option<Dimensions> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs())
        .unwrap_or(0);
    let key = (path.to_path_buf(), mtime, meta.len());

    if let Some(hit) = dimension_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return *hit;
    }

    let measured = read_dimensions(path);
    dimension_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, measured);
    measured
}

/// Header-only read — never decodes pixels.
fn read_dimensions(path: &Path) -> Option<Dimensions> {
    let reader = image::io::Reader::open(path).ok()?;
    let reader = match reader.format() {
        Some(_) => reader,
        None => reader.with_guessed_format().ok()?,
    };
    reader.into_dimensions().ok()
}

/// The asset's resolution as last measured, for display. Populated by `run`.
pub fn measured(key: &RowKey) -> Option<Dimensions> {
    *measured_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)?
}

/// A short human label for a width: `16384` -> `"16k"`.
pub fn label(width: u32) -> Option<String> {
    match width {
        0 => None,
        w if w % 1024 == 0 => Some(format!("{}k", w / 1024)),
        w if w > 1024 => Some(format!("{:.1}k", w as f32 / 1024.0)),
        w => Some(format!("{w}px")),
    }
}

/// The HDRI whose header defines the asset's resolution: the Prod staging file,
/// falling back to Local so a work-in-progress asset still reports one.
pub(crate) fn hdri_source(ctx: &ValidationContext) -> Option<PathBuf> {
    staging_hdri(&ctx.prod_root, &ctx.key.slug)
        .or_else(|| staging_hdri(&ctx.local_root, &ctx.key.slug))
}

fn staging_hdri(root: &Path, slug: &str) -> Option<PathBuf> {
    let staging = root.join("staging");
    let exr = staging.join(format!("{slug}.exr"));
    if exr.is_file() {
        return Some(exr);
    }
    let hdr = staging.join(format!("{slug}.hdr"));
    hdr.is_file().then_some(hdr)
}

/// Every image under a Texture/Model's `staging/textures`, preferring Prod.
fn texture_images(ctx: &ValidationContext) -> Vec<PathBuf> {
    for root in [&ctx.prod_root, &ctx.local_root] {
        let dir = root.join("staging").join("textures");
        if !dir.is_dir() {
            continue;
        }
        let mut images: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| is_image(path))
            .collect();
        if !images.is_empty() {
            images.sort();
            return images;
        }
    }
    Vec::new()
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
        .unwrap_or(false)
}

/// Measure the asset, remember the result for the UI, and — for Textures and
/// Models — report images that disagree on a resolution.
pub(crate) fn run(ctx: &ValidationContext) -> Vec<Finding> {
    let (measured, findings) = match ctx.key.asset_type {
        AssetType::Hdris => (
            hdri_source(ctx).and_then(|path| dimensions(&path)),
            Vec::new(),
        ),
        AssetType::Textures | AssetType::Models => measure_textures(ctx),
    };

    measured_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(ctx.key.clone(), measured);

    findings
}

/// All of a Texture/Model's images must share one resolution.
fn measure_textures(ctx: &ValidationContext) -> (Option<Dimensions>, Vec<Finding>) {
    let images = texture_images(ctx);
    if images.is_empty() {
        return (None, Vec::new());
    }

    // Group by resolution, keeping the file order for a stable message.
    let mut groups: Vec<(Dimensions, Vec<String>)> = Vec::new();
    for path in &images {
        let Some(size) = dimensions(path) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        match groups.iter_mut().find(|(existing, _)| *existing == size) {
            Some((_, names)) => names.push(name),
            None => groups.push((size, vec![name])),
        }
    }

    // The resolution to display: whichever the most images agree on.
    let representative = groups
        .iter()
        .max_by_key(|(_, names)| names.len())
        .map(|(size, _)| *size);

    if groups.len() < 2 {
        return (representative, Vec::new());
    }

    // Biggest group first, so the odd ones out read as the exceptions.
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(b.0.cmp(&a.0)));
    let summary = groups
        .iter()
        .map(|((width, height), names)| format!("{width}x{height} ({})", names.len()))
        .collect::<Vec<_>>()
        .join(", ");
    let odd_ones_out = groups
        .iter()
        .skip(1)
        .flat_map(|(_, names)| names.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");

    (
        representative,
        vec![Finding {
            severity: Severity::Error,
            text: format!(
                "Mixed image resolutions in staging/textures: {summary} — check {odd_ones_out}"
            ),
            dismiss_id: None,
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_use_k_for_whole_multiples_of_1024() {
        assert_eq!(label(24576).as_deref(), Some("24k"));
        assert_eq!(label(16384).as_deref(), Some("16k"));
        assert_eq!(label(8192).as_deref(), Some("8k"));
        assert_eq!(label(1024).as_deref(), Some("1k"));
    }

    #[test]
    fn labels_do_not_round_away_an_odd_resolution() {
        assert_eq!(label(1536).as_deref(), Some("1.5k"));
        assert_eq!(label(512).as_deref(), Some("512px"));
        assert_eq!(label(0), None);
    }

    #[test]
    fn only_readable_image_extensions_are_measured() {
        assert!(is_image(Path::new("a/diffuse.png")));
        assert!(is_image(Path::new("a/DIFFUSE.PNG")));
        assert!(is_image(Path::new("a/x.exr")));
        assert!(!is_image(Path::new("a/notes.txt")));
        assert!(!is_image(Path::new("a/source.tif")));
        assert!(!is_image(Path::new("a/no_extension")));
    }

    fn texture_asset(sizes: &[(&str, u32, u32)]) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let prod = temp.path().join("prod");
        let dir = prod.join("staging").join("textures");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, w, h) in sizes {
            image::RgbImage::new(*w, *h).save(dir.join(name)).unwrap();
        }
        (temp, prod)
    }

    #[test]
    fn textures_that_agree_on_a_resolution_pass_and_are_measured() {
        let slug = "uniform_texture";
        let (temp, prod) = texture_asset(&[
            ("diff.png", 64, 64),
            ("nor.png", 64, 64),
            ("ao.png", 64, 64),
        ]);

        let findings = crate::validation::validate_asset(
            AssetType::Textures,
            slug,
            None,
            &[],
            &temp.path().join("local"),
            &prod,
        );

        assert!(!findings
            .iter()
            .any(|f| f.text.contains("Mixed image resolutions")));
        assert_eq!(
            measured(&RowKey {
                asset_type: AssetType::Textures,
                slug: slug.into()
            }),
            Some((64, 64))
        );
    }

    #[test]
    fn textures_that_disagree_on_a_resolution_are_a_hard_error() {
        let slug = "mixed_texture";
        let (temp, prod) = texture_asset(&[
            ("diff.png", 64, 64),
            ("nor.png", 64, 64),
            ("rough.png", 32, 32),
        ]);

        let findings = crate::validation::validate_asset(
            AssetType::Textures,
            slug,
            None,
            &[],
            &temp.path().join("local"),
            &prod,
        );

        let mixed = findings
            .iter()
            .find(|f| f.text.contains("Mixed image resolutions"))
            .expect("mixed resolutions should be reported");
        assert_eq!(mixed.severity, Severity::Error);
        assert!(mixed.dismiss_id.is_none(), "this one is not dismissable");
        assert!(mixed.text.contains("64x64 (2)"));
        assert!(mixed.text.contains("32x32 (1)"));
        // Names the odd one out, not the majority.
        assert!(mixed.text.contains("rough.png"));
        assert!(!mixed.text.contains("diff.png"));
        // The majority resolution is still what gets displayed.
        assert_eq!(
            measured(&RowKey {
                asset_type: AssetType::Textures,
                slug: slug.into()
            }),
            Some((64, 64))
        );
    }

    #[test]
    fn models_are_held_to_the_same_rule_as_textures() {
        let slug = "mixed_model";
        let (temp, prod) = texture_asset(&[("a.png", 64, 64), ("b.png", 16, 16)]);

        let findings = crate::validation::validate_asset(
            AssetType::Models,
            slug,
            None,
            &[],
            &temp.path().join("local"),
            &prod,
        );

        assert!(findings
            .iter()
            .any(|f| f.text.contains("Mixed image resolutions")));
    }

    #[test]
    fn an_asset_with_no_images_reports_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let findings = crate::validation::validate_asset(
            AssetType::Textures,
            "empty_texture",
            None,
            &[],
            &temp.path().join("local"),
            &temp.path().join("prod"),
        );
        assert!(findings.is_empty());
    }

    #[test]
    fn dimensions_are_cached_per_file_revision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("probe.png");
        image::RgbImage::new(64, 32).save(&path).unwrap();

        assert_eq!(dimensions(&path), Some((64, 32)));
        // Second read comes from the memo — deleting the file can't change it.
        std::fs::remove_file(&path).unwrap();
        image::RgbImage::new(64, 32).save(&path).unwrap();
        assert_eq!(dimensions(&path), Some((64, 32)));
    }
}
