//! The per-card record of what has already been ingested and verified.
//!
//! Without this, re-opening a finished card would mean re-reading ~100 GB from both the
//! card and the NAS just to discover that nothing changed. The manifest records the BLAKE3
//! that was verified at copy time, so a re-run only has to confirm that neither side has
//! been touched since.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::scan::IngestFile;

/// Same tolerance the rest of the codebase uses (`copy::plan::MTIME_TOLERANCE_SECS`).
/// exFAT and FAT32 do not store timestamps precisely enough for exact comparison, and the
/// NAS may round what it stores.
const MTIME_TOLERANCE_SECS: i64 = 2;

/// Largest whole-hour timestamp shift still treated as a timezone artefact rather than an
/// edit. FAT-family filesystems store local time with no zone, so an untouched file reads
/// an hour out after a DST change, or several hours out on a machine set to another zone.
const MAX_TIMEZONE_SHIFT_SECS: i64 = 25 * 3600;

const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub src_size: u64,
    pub src_mtime: i64,
    pub dst_size: u64,
    pub dst_mtime: i64,
    /// BLAKE3 of the file, verified against the destination after copying.
    pub blake3: String,
    /// Thumbnail path relative to the card's ingest directory, if one could be made.
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default)]
    pub camera: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// The card's opaque signature — `friendly_name` is only a display label, so this is
    /// what proves the folder belongs to the card being ingested.
    #[serde(default)]
    pub card_signature: String,
    #[serde(default)]
    pub card_name: String,
    /// Keyed by relative path with forward slashes, so a manifest stays readable and
    /// portable regardless of which machine wrote it.
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            card_signature: String::new(),
            card_name: String::new(),
            entries: BTreeMap::new(),
        }
    }
}

/// Whether a file still matches what the manifest says was ingested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Both sides match what was recorded: skip it entirely, no I/O.
    Current,
    /// Sizes match but a timestamp moved by a whole number of hours — almost certainly a
    /// timezone or DST artefact on a FAT-family card, not an edit. Confirm by hashing the
    /// source rather than re-copying ~100 GB.
    Recheck,
    /// Genuinely different, missing, or never ingested.
    Stale,
}

/// Key for a file within the manifest.
pub fn key_for(rel_path: &Path) -> String {
    rel_path.to_string_lossy().replace('\\', "/")
}

impl Manifest {
    pub fn path_in(card_dir: &Path) -> PathBuf {
        card_dir.join(super::MANIFEST_NAME)
    }

    /// Load a card's manifest, falling back to the backup the atomic write leaves behind
    /// and finally to an empty manifest. A missing or corrupt manifest costs a re-verify,
    /// never correctness, so it is never an error.
    pub fn load(card_dir: &Path) -> Self {
        let path = Self::path_in(card_dir);
        for candidate in [path.clone(), backup_path(&path)] {
            let Ok(text) = std::fs::read_to_string(&candidate) else {
                continue;
            };
            match serde_json::from_str::<Manifest>(&text) {
                Ok(manifest) => return manifest,
                Err(err) => log::warn!("Ignoring unreadable {}: {err}", candidate.display()),
            }
        }
        Self::default()
    }

    pub fn save(&self, card_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(card_dir)?;
        let text = serde_json::to_string_pretty(self)?;
        crate::config::save_text_atomically(&Self::path_in(card_dir), &text)
    }

    pub fn get(&self, rel_path: &Path) -> Option<&Entry> {
        self.entries.get(&key_for(rel_path))
    }

    pub fn insert(&mut self, rel_path: &Path, entry: Entry) {
        self.entries.insert(key_for(rel_path), entry);
    }

    /// Decide whether `file` still needs work, given what is on the destination now.
    ///
    /// `dst` is the destination's `(size, mtime)`, or `None` if it does not exist.
    pub fn freshness(&self, file: &IngestFile, dst: Option<(u64, i64)>) -> Freshness {
        let Some(entry) = self.get(&file.rel_path) else {
            return Freshness::Stale;
        };
        let Some((dst_size, dst_mtime)) = dst else {
            return Freshness::Stale;
        };
        // Size is the one field no filesystem rounds or reinterprets, so a mismatch on
        // either side is decisive.
        if entry.src_size != file.size || entry.dst_size != dst_size {
            return Freshness::Stale;
        }
        // The destination is ours and lives on a normal filesystem; if its timestamp moved
        // at all, something outside PHASE touched it.
        if !within_tolerance(entry.dst_mtime, dst_mtime) {
            return Freshness::Stale;
        }
        if within_tolerance(entry.src_mtime, file.mtime) {
            return Freshness::Current;
        }
        if is_timezone_shift(entry.src_mtime, file.mtime) {
            return Freshness::Recheck;
        }
        Freshness::Stale
    }
}

fn within_tolerance(a: i64, b: i64) -> bool {
    a.abs_diff(b) <= MTIME_TOLERANCE_SECS as u64
}

/// A difference that is a whole number of hours (within the usual sloppiness) and no more
/// than a day — the signature of a timezone or DST reinterpretation rather than an edit.
fn is_timezone_shift(a: i64, b: i64) -> bool {
    let delta = a.abs_diff(b);
    if delta == 0 || delta > MAX_TIMEZONE_SHIFT_SECS as u64 {
        return false;
    }
    // Some zones are offset by half or quarter hours, so accept any multiple of 15 minutes.
    let quarter = 15 * 60;
    let remainder = delta % quarter;
    remainder <= MTIME_TOLERANCE_SECS as u64 || quarter - remainder <= MTIME_TOLERANCE_SECS as u64
}

fn backup_path(path: &Path) -> PathBuf {
    let mut raw = path.as_os_str().to_owned();
    raw.push(".bak");
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(mtime: i64, size: u64) -> IngestFile {
        IngestFile {
            rel_path: PathBuf::from("DCIM").join("100").join("IMG_1.NEF"),
            src_abs: PathBuf::from(r"I:\DCIM\100\IMG_1.NEF"),
            size,
            mtime,
        }
    }

    fn manifest_with(entry: Entry) -> Manifest {
        let mut manifest = Manifest::default();
        manifest.insert(&file(0, 0).rel_path, entry);
        manifest
    }

    fn entry(src_mtime: i64, size: u64) -> Entry {
        Entry {
            src_size: size,
            src_mtime,
            dst_size: size,
            dst_mtime: src_mtime,
            blake3: "abc123".into(),
            thumb: Some("thumbs/DCIM/100/IMG_1.NEF.jpg".into()),
            camera: Some("Nikon Z 7".into()),
        }
    }

    #[test]
    fn an_untouched_pair_is_current() {
        let manifest = manifest_with(entry(1_700_000_000, 50));
        assert_eq!(
            manifest.freshness(&file(1_700_000_000, 50), Some((50, 1_700_000_000))),
            Freshness::Current
        );
    }

    #[test]
    fn sub_second_rounding_is_tolerated_on_both_sides() {
        let manifest = manifest_with(entry(1_700_000_000, 50));
        // exFAT and the NAS both round; ±2s must not force a re-copy.
        assert_eq!(
            manifest.freshness(&file(1_700_000_002, 50), Some((50, 1_699_999_998))),
            Freshness::Current
        );
    }

    #[test]
    fn a_missing_or_unknown_destination_is_stale() {
        let manifest = manifest_with(entry(1_700_000_000, 50));
        assert_eq!(manifest.freshness(&file(1_700_000_000, 50), None), Freshness::Stale);
        assert_eq!(
            Manifest::default().freshness(&file(1_700_000_000, 50), Some((50, 1_700_000_000))),
            Freshness::Stale
        );
    }

    #[test]
    fn any_size_change_is_stale() {
        let manifest = manifest_with(entry(1_700_000_000, 50));
        assert_eq!(
            manifest.freshness(&file(1_700_000_000, 51), Some((50, 1_700_000_000))),
            Freshness::Stale
        );
        assert_eq!(
            manifest.freshness(&file(1_700_000_000, 50), Some((49, 1_700_000_000))),
            Freshness::Stale
        );
    }

    #[test]
    fn a_touched_destination_is_stale_even_if_the_source_is_unchanged() {
        let manifest = manifest_with(entry(1_700_000_000, 50));
        assert_eq!(
            manifest.freshness(&file(1_700_000_000, 50), Some((50, 1_700_000_900))),
            Freshness::Stale
        );
    }

    #[test]
    fn a_whole_hour_shift_on_the_card_asks_for_a_hash_rather_than_a_recopy() {
        // FAT-family cards store local time with no zone, so a DST change moves every
        // timestamp by an hour without a byte having changed. Re-copying 100 GB over that
        // would be absurd; re-hashing the source settles it.
        let manifest = manifest_with(entry(1_700_000_000, 50));
        for shift in [3600, -3600, 7200, -7200, 12 * 3600] {
            assert_eq!(
                manifest.freshness(&file(1_700_000_000 + shift, 50), Some((50, 1_700_000_000))),
                Freshness::Recheck,
                "shift of {shift}s should be treated as a timezone artefact"
            );
        }
        // Half-hour zones exist too (India, parts of Australia).
        assert_eq!(
            manifest.freshness(&file(1_700_000_000 + 1800, 50), Some((50, 1_700_000_000))),
            Freshness::Recheck
        );
    }

    #[test]
    fn an_arbitrary_timestamp_change_is_stale_not_a_timezone_artefact() {
        let manifest = manifest_with(entry(1_700_000_000, 50));
        for shift in [60, 600, 3000, 2 * 24 * 3600] {
            assert_eq!(
                manifest.freshness(&file(1_700_000_000 + shift, 50), Some((50, 1_700_000_000))),
                Freshness::Stale,
                "shift of {shift}s should not be excused"
            );
        }
    }

    #[test]
    fn keys_are_portable_across_path_separators() {
        assert_eq!(key_for(Path::new(r"DCIM\115NCZ_7\PHC_2730.NEF")), "DCIM/115NCZ_7/PHC_2730.NEF");
        assert_eq!(key_for(Path::new("DCIM/115NCZ_7/PHC_2730.NEF")), "DCIM/115NCZ_7/PHC_2730.NEF");
    }

    #[test]
    fn a_manifest_survives_a_round_trip_to_disk() {
        let temp = tempfile::tempdir().unwrap();
        let mut manifest = Manifest {
            card_signature: "671658a1b2c3d4e5".into(),
            card_name: "120GB_671658".into(),
            ..Default::default()
        };
        manifest.insert(Path::new(r"DCIM\100\IMG_1.NEF"), entry(1_700_000_000, 50));
        manifest.save(temp.path()).unwrap();

        let loaded = Manifest::load(temp.path());
        assert_eq!(loaded.card_signature, "671658a1b2c3d4e5");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            loaded.get(Path::new("DCIM/100/IMG_1.NEF")).unwrap().blake3,
            "abc123"
        );
    }

    #[test]
    fn a_corrupt_manifest_falls_back_rather_than_failing() {
        let temp = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::default();
        manifest.insert(Path::new("a.nef"), entry(1, 1));
        manifest.save(temp.path()).unwrap();
        // Saving twice leaves a .bak the corrupt-primary path can fall back to.
        manifest.insert(Path::new("b.nef"), entry(2, 2));
        manifest.save(temp.path()).unwrap();

        std::fs::write(Manifest::path_in(temp.path()), "{ not json").unwrap();
        assert_eq!(Manifest::load(temp.path()).entries.len(), 1);

        // No manifest at all is simply an empty one.
        let empty = tempfile::tempdir().unwrap();
        assert!(Manifest::load(empty.path()).entries.is_empty());
    }
}
