use crate::ui::AssetType;
use crate::validation::resolution;
use crate::validation::{Finding, Severity, ValidationContext};

/// HDRIs are delivered at 16k wide.
const MIN_WIDTH: u32 = 16384;
/// ...and at a height that is a whole number of 1024px bands (e.g. 8192, 12288).
const HEIGHT_MULTIPLE: u32 = 1024;

pub(crate) fn run(ctx: &ValidationContext) -> Vec<Finding> {
    if !matches!(ctx.key.asset_type, AssetType::Hdris) {
        return Vec::new();
    }
    let Some(path) = resolution::hdri_source(ctx) else {
        return Vec::new();
    };
    // An unreadable/exotic file yields no dimensions; stay quiet rather than
    // reporting a resolution problem we cannot actually prove. The read is
    // memoised, so this shares one header read with `resolution`.
    let Some((width, height)) = resolution::dimensions(&path) else {
        return Vec::new();
    };
    findings_for_dimensions(width, height)
}

/// The resolution rules, split out so they can be tested without a real image.
fn findings_for_dimensions(width: u32, height: u32) -> Vec<Finding> {
    let mut findings = Vec::new();
    if width < MIN_WIDTH {
        findings.push(Finding {
            severity: Severity::Warning,
            text: format!("HDRI is {width}px wide; expected at least {MIN_WIDTH}px"),
            dismiss_id: Some("hdri-width-too-small"),
        });
    }
    if !height.is_multiple_of(HEIGHT_MULTIPLE) {
        findings.push(Finding {
            severity: Severity::Warning,
            text: format!("HDRI is {height}px high; expected a multiple of {HEIGHT_MULTIPLE}px"),
            dismiss_id: Some("hdri-height-not-multiple"),
        });
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_size_hdri_has_no_findings() {
        assert!(findings_for_dimensions(16384, 8192).is_empty());
        assert!(findings_for_dimensions(24576, 12288).is_empty());
    }

    #[test]
    fn narrow_hdri_is_flagged() {
        let findings = findings_for_dimensions(8192, 4096);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].text.contains("8192px wide"));
        assert!(findings[0].text.contains("16384"));
    }

    #[test]
    fn height_that_is_not_a_multiple_of_1024_is_flagged() {
        let findings = findings_for_dimensions(16384, 8000);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].text.contains("8000px high"));
        assert!(findings[0].text.contains("1024"));
    }

    #[test]
    fn both_problems_are_reported_together() {
        let findings = findings_for_dimensions(4096, 1234);
        assert_eq!(findings.len(), 2);
    }

    /// Proves the header read works end to end for the formats HDRIs ship in.
    #[test]
    fn dimensions_are_read_from_exr_and_hdr_headers() {
        let temp = tempfile::tempdir().unwrap();
        let pixels = image::Rgb32FImage::from_pixel(64, 32, image::Rgb([0.5f32, 0.5, 0.5]));

        let exr = temp.path().join("probe.exr");
        pixels.save(&exr).unwrap();
        assert_eq!(resolution::dimensions(&exr), Some((64, 32)));

        // `save()` has no Radiance encoder, so write that one directly.
        let hdr = temp.path().join("probe.hdr");
        let texels = vec![image::Rgb([0.5f32, 0.5, 0.5]); 64 * 32];
        image::codecs::hdr::HdrEncoder::new(std::io::BufWriter::new(
            std::fs::File::create(&hdr).unwrap(),
        ))
        .encode(&texels, 64, 32)
        .unwrap();
        assert_eq!(resolution::dimensions(&hdr), Some((64, 32)));
    }

    #[test]
    fn undersized_staging_exr_is_reported_for_hdris_only() {
        let temp = tempfile::tempdir().unwrap();
        let prod = temp.path().join("prod");
        std::fs::create_dir_all(prod.join("staging")).unwrap();
        image::Rgb32FImage::from_pixel(64, 30, image::Rgb([0.5f32, 0.5, 0.5]))
            .save(prod.join("staging/sunny_field.exr"))
            .unwrap();

        let findings = crate::validation::validate_asset(
            AssetType::Hdris,
            "sunny_field",
            None,
            &[],
            &temp.path().join("local"),
            &prod,
        );
        assert!(findings.iter().any(|f| f.text.contains("64px wide")));
        assert!(findings.iter().any(|f| f.text.contains("30px high")));

        // Textures/Models have no such rule.
        let findings = crate::validation::validate_asset(
            AssetType::Textures,
            "sunny_field",
            None,
            &[],
            &temp.path().join("local"),
            &prod,
        );
        assert!(!findings.iter().any(|f| f.text.contains("px wide")));
    }
}
