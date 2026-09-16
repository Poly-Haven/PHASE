//! Memory-card ingest: copy every still image off a card into
//! `{ingest_root}\{card}\`, hash-verified, with a thumbnail per file.
//!
//! Cards are usually formatted right after a shoot, so the ingested copy is frequently the
//! only copy that will ever exist. Everything here is shaped by that: verify what actually
//! landed on the far end, decode each RAW to prove its payload survived, never abort a run
//! because one file went bad, never drop a file without counting it, and leave a log on the
//! card saying whether it is safe to format.

pub mod exif;
pub mod job;
pub mod log;
pub mod manifest;
pub mod scan;
pub mod thumb;

use std::path::{Path, PathBuf};

use crate::removable_media::CardFingerprint;

/// Camera RAW formats. Deliberately the inverse of `copy::plan::PULL_EXCLUDED_EXT`, which
/// lists the same formats as things *not* to pull; a test keeps the two from drifting.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw",
    "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// Non-RAW stills worth ingesting.
///
/// `bmp` is deliberately absent: no camera shoots BMP, but Magic Lantern installs a folder
/// full of `.bmp` UI assets on the card, and those are not photographs.
pub const STILL_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp"];

/// Directory names that are never worth walking into.
pub const SKIP_DIRS: &[&str] = &[
    "system volume information",
    "$recycle.bin",
    ".trashes",
    ".spotlight-v100",
    ".fseventsd",
];

/// The summary PHASE leaves at the root of an ingested card.
pub const CARD_LOG_NAME: &str = "phase_log.txt";

/// Per-card manifest, stored alongside the ingested files.
pub const MANIFEST_NAME: &str = ".phase-ingest.json";

/// Subdirectory of a card's ingest folder holding generated thumbnails.
pub const THUMBS_DIR: &str = "thumbs";

/// Lowercased extension of a file name, without the dot. Empty when there is none.
pub fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn is_raw_extension(ext: &str) -> bool {
    RAW_EXTENSIONS.contains(&ext)
}

pub fn is_still_extension(ext: &str) -> bool {
    STILL_EXTENSIONS.contains(&ext)
}

/// Whether a file is something ingest should copy at all.
pub fn is_ingestable_extension(ext: &str) -> bool {
    is_raw_extension(ext) || is_still_extension(ext)
}

/// Where a card's files land.
pub fn card_dir(ingest_root: &Path, card: &CardFingerprint) -> PathBuf {
    ingest_root.join(&card.friendly_name)
}

/// Thumbnail path for an ingested file, mirroring its relative path under `thumbs\`.
///
/// The source extension is kept and `.jpg` appended rather than replaced, so an ingested
/// `IMG_0001.NEF` and `IMG_0001.JPG` could never collide on one thumbnail.
pub fn thumb_rel_path(rel_path: &Path) -> PathBuf {
    let mut name = rel_path.file_name().unwrap_or_default().to_os_string();
    name.push(".jpg");
    PathBuf::from(THUMBS_DIR)
        .join(rel_path.parent().unwrap_or(Path::new("")))
        .join(name)
}

/// Present a camera as a person would write it.
///
/// Bodies report SHOUTED makes and bare model codes (`NIKON CORPORATION` / `NIKON Z 7`,
/// `SONY` / `ILCE-7R`), so: drop a make the model already repeats, and gently title-case
/// shouted *words* while leaving anything containing a digit alone — `ILCE-7R` and `600D`
/// are model codes, not prose, and `EOS` is short enough that lowercasing it looks wrong.
pub fn format_camera(make: &str, model: &str) -> String {
    let make = make.trim();
    let model = model.trim();
    let first_make_word = make.split_whitespace().next().unwrap_or("");

    let combined = if model.is_empty() {
        make.to_string()
    } else if first_make_word.is_empty()
        || model
            .to_ascii_lowercase()
            .starts_with(&first_make_word.to_ascii_lowercase())
    {
        model.to_string()
    } else {
        format!("{first_make_word} {model}")
    };

    combined
        .split_whitespace()
        .map(soften_shouted_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn soften_shouted_word(word: &str) -> String {
    let shouted = word.chars().any(|c| c.is_ascii_uppercase())
        && !word.chars().any(|c| c.is_ascii_lowercase());
    let is_prose = word.chars().all(|c| c.is_ascii_alphabetic()) && word.len() > 3;
    if !shouted || !is_prose {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_list_covers_every_raw_format_pull_excludes() {
        // Both lists name the same family of files for opposite reasons; if one grows a
        // format the other must too.
        for ext in crate::copy::plan::PULL_EXCLUDED_EXT {
            // `tif`/`tiff` and `pp3` are excluded from pull for unrelated reasons.
            if matches!(*ext, "tif" | "tiff" | "pp3") {
                continue;
            }
            assert!(
                is_raw_extension(ext),
                "{ext} is treated as RAW by pull but not by ingest"
            );
        }
    }

    #[test]
    fn magic_lantern_assets_are_not_photographs() {
        for ext in ["bmp", "lua", "mo", "rbf", "sym", "cfg", "lut", "dat", "fir"] {
            assert!(!is_ingestable_extension(ext), "{ext} should be skipped");
        }
    }

    #[test]
    fn video_and_sidecars_are_not_ingested() {
        for ext in ["mov", "mp4", "avi", "mts", "m4v", "xmp", "thm", "lrc", "wav"] {
            assert!(!is_ingestable_extension(ext), "{ext} should be skipped");
        }
    }

    #[test]
    fn extensions_are_matched_case_insensitively() {
        assert_eq!(extension_of("DSC03798.ARW"), "arw");
        assert_eq!(extension_of("PHC_2730.NEF"), "nef");
        assert!(is_raw_extension(&extension_of("PHC_2730.NEF")));
        assert!(is_still_extension(&extension_of("DSC03798.JPG")));
        assert_eq!(extension_of("NOEXTENSION"), "");
        assert!(!is_ingestable_extension(&extension_of("NOEXTENSION")));
    }

    #[test]
    fn camera_names_read_like_a_person_wrote_them() {
        assert_eq!(format_camera("NIKON CORPORATION", "NIKON Z 7"), "Nikon Z 7");
        assert_eq!(format_camera("SONY", "ILCE-7R"), "Sony ILCE-7R");
        assert_eq!(format_camera("Canon", "Canon EOS 600D"), "Canon EOS 600D");
        assert_eq!(format_camera("CANON", "CANON EOS 600D"), "Canon EOS 600D");
        assert_eq!(format_camera("FUJIFILM", "X-T4"), "Fujifilm X-T4");
        // Degenerate halves must not produce stray whitespace.
        assert_eq!(format_camera("", "ILCE-7R"), "ILCE-7R");
        assert_eq!(format_camera("SONY", ""), "Sony");
        assert_eq!(format_camera("", ""), "");
    }

    #[test]
    fn thumbnails_mirror_the_card_tree_without_colliding() {
        assert_eq!(
            thumb_rel_path(Path::new("DCIM/115NCZ_7/PHC_2730.NEF")),
            PathBuf::from("thumbs/DCIM/115NCZ_7/PHC_2730.NEF.jpg")
        );
        // Same stem, different source format: distinct thumbnails.
        assert_ne!(
            thumb_rel_path(Path::new("DCIM/100/IMG_1.NEF")),
            thumb_rel_path(Path::new("DCIM/100/IMG_1.JPG"))
        );
        // A file at the card root still lands under thumbs/.
        assert_eq!(
            thumb_rel_path(Path::new("LOOSE.JPG")),
            PathBuf::from("thumbs/LOOSE.JPG.jpg")
        );
    }
}
