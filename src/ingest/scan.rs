//! Walking a card and deciding what to ingest.
//!
//! Cards are messy in ways that matter. A body writes RAW and JPEG side by side; a card
//! formatted in one camera gets used in another; firmware hacks install whole directory
//! trees of their own assets. The `EOS_DIGITAL`-labelled test card holds Sony ARWs in
//! `DCIM\101MSDCF`, an empty `100CANON`, an AVCHD video tree, and a Magic Lantern install
//! complete with `.bmp` UI graphics — none of which are photographs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use filetime::FileTime;
use walkdir::WalkDir;

use super::exif;

/// How many sample groups we are willing to read camera headers from. Even a pathological
/// card should be a handful; this only stops a card with thousands of one-file directories
/// from turning the scan into thousands of reads.
const MAX_CAMERA_SAMPLES: usize = 24;

/// One file that will be ingested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestFile {
    /// Path relative to the card root — the identity of the file everywhere downstream.
    pub rel_path: PathBuf,
    pub src_abs: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

/// What the scan chose not to ingest, so nothing disappears silently from a card that is
/// about to be formatted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Skipped {
    /// JPEGs dropped because the same shot is present as a RAW.
    pub jpeg_with_raw: usize,
    /// Everything else: video, sidecars, firmware, index files.
    pub other: usize,
}

impl Skipped {
    /// Human-readable summary, or `None` when nothing was skipped.
    pub fn describe(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.jpeg_with_raw > 0 {
            parts.push(format!("{} JPEG with RAW", self.jpeg_with_raw));
        }
        if self.other > 0 {
            parts.push(format!("{} non-image", self.other));
        }
        (!parts.is_empty()).then(|| format!("skipped {}", parts.join(", ")))
    }
}

/// Everything the ingest screen needs to describe a card.
#[derive(Debug, Default, Clone)]
pub struct CardScan {
    pub files: Vec<IngestFile>,
    /// Upper-case extension counts in descending order, e.g. `[("NEF", 2120)]`.
    pub ext_counts: Vec<(String, usize)>,
    pub cameras: Vec<String>,
    pub skipped: Skipped,
}

impl CardScan {
    /// e.g. `2120 NEF, 17 CR2`.
    pub fn describe_contents(&self) -> String {
        self.ext_counts
            .iter()
            .map(|(ext, count)| format!("{count} {ext}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Walk `root` and decide what to ingest. `scanned` is bumped per file examined so the UI
/// can show progress on a card with thousands of files.
pub fn scan(root: &Path, scanned: &AtomicUsize) -> CardScan {
    let (candidates, other) = collect(root, scanned);
    let (files, jpeg_with_raw) = drop_jpegs_shadowed_by_raw(candidates);

    let mut ext_counts: HashMap<String, usize> = HashMap::new();
    for file in &files {
        let ext = super::extension_of(&file.rel_path.to_string_lossy()).to_ascii_uppercase();
        *ext_counts.entry(ext).or_default() += 1;
    }
    let mut ext_counts: Vec<(String, usize)> = ext_counts.into_iter().collect();
    // Commonest first, then alphabetically so the header text is stable between scans.
    ext_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let cameras = detect_cameras(&files);

    CardScan {
        skipped: Skipped {
            jpeg_with_raw,
            other,
        },
        files,
        ext_counts,
        cameras,
    }
}

/// Walk the card, returning every still image plus a count of everything ignored.
fn collect(root: &Path, scanned: &AtomicUsize) -> (Vec<IngestFile>, usize) {
    let mut files = Vec::new();
    let mut other = 0usize;

    let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|entry| {
        if entry.depth() == 0 || !entry.file_type().is_dir() {
            return true;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        !super::SKIP_DIRS.contains(&name.as_str())
    });

    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        scanned.fetch_add(1, Ordering::Relaxed);

        let name = entry.file_name().to_string_lossy().to_string();
        // Our own log must never ingest itself.
        if name.eq_ignore_ascii_case(super::CARD_LOG_NAME) {
            continue;
        }
        if !super::is_ingestable_extension(&super::extension_of(&name)) {
            other += 1;
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            // Unreadable metadata means we cannot plan the copy; count it rather than
            // pretending the file was not there.
            other += 1;
            continue;
        };
        let Ok(rel_path) = entry.path().strip_prefix(root) else {
            continue;
        };
        files.push(IngestFile {
            rel_path: rel_path.to_path_buf(),
            src_abs: entry.path().to_path_buf(),
            size: metadata.len(),
            mtime: FileTime::from_last_modification_time(&metadata).unix_seconds(),
        });
    }

    // Stable, predictable order: this is the order the grid draws and workers consume.
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    (files, other)
}

/// Drop JPEGs that are just the camera's preview of a RAW sitting beside them.
///
/// Scoped to a single directory on purpose: two image folders on one card may legitimately
/// hold *different* shots under the same file name, so matching stems across directories
/// would discard real photographs.
fn drop_jpegs_shadowed_by_raw(files: Vec<IngestFile>) -> (Vec<IngestFile>, usize) {
    let mut raw_stems: HashMap<(PathBuf, String), ()> = HashMap::new();
    for file in &files {
        let ext = super::extension_of(&file.rel_path.to_string_lossy());
        if super::is_raw_extension(&ext) {
            raw_stems.insert(stem_key(&file.rel_path), ());
        }
    }

    let mut kept = Vec::with_capacity(files.len());
    let mut dropped = 0usize;
    for file in files {
        let ext = super::extension_of(&file.rel_path.to_string_lossy());
        if !super::is_raw_extension(&ext) && raw_stems.contains_key(&stem_key(&file.rel_path)) {
            dropped += 1;
            continue;
        }
        kept.push(file);
    }
    (kept, dropped)
}

fn stem_key(rel_path: &Path) -> (PathBuf, String) {
    let parent = rel_path.parent().unwrap_or(Path::new("")).to_path_buf();
    let stem = rel_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    (parent, stem)
}

/// Read camera names from a handful of representative files rather than all of them.
///
/// Reading a header from every file would be hundreds of megabytes of random reads and
/// thousands of opens before the user can click anything, to render one line of text.
/// Instead we sample one file per *group*, where a group is a run of consecutively
/// numbered files sharing a directory, name prefix and extension — `DSC03798.ARW` through
/// `DSC03912.ARW` cannot have come from two different bodies, but a gap in the numbering,
/// a different prefix or a different format all plausibly mean a second camera.
fn detect_cameras(files: &[IngestFile]) -> Vec<String> {
    let mut cameras = Vec::new();
    for file in sample_files(files) {
        let Some(header) = read_header(&file.src_abs) else {
            continue;
        };
        if let Some(camera) = header.camera() {
            if !cameras.contains(&camera) {
                cameras.push(camera);
            }
        }
    }
    cameras
}

/// One representative file per group, in file order.
fn sample_files(files: &[IngestFile]) -> Vec<&IngestFile> {
    // Group by directory + alphabetic prefix + extension, keeping the numeric part so runs
    // can be found within each group.
    let mut groups: HashMap<(PathBuf, String, String), Vec<(u64, usize)>> = HashMap::new();
    for (index, file) in files.iter().enumerate() {
        let name = file.rel_path.file_name().unwrap_or_default().to_string_lossy();
        let stem = file
            .rel_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let (prefix, number) = split_sequence(&stem);
        let key = (
            file.rel_path.parent().unwrap_or(Path::new("")).to_path_buf(),
            prefix,
            super::extension_of(&name),
        );
        groups.entry(key).or_default().push((number, index));
    }

    let mut sampled_indices = Vec::new();
    for mut numbered in groups.into_values() {
        numbered.sort_unstable();
        let mut previous: Option<u64> = None;
        for (number, index) in numbered {
            // A break in the sequence starts a new run, and so earns its own sample.
            let continues = previous.is_some_and(|p| number == p || number == p + 1);
            if !continues {
                sampled_indices.push(index);
            }
            previous = Some(number);
        }
    }

    sampled_indices.sort_unstable();
    sampled_indices.truncate(MAX_CAMERA_SAMPLES);
    sampled_indices.into_iter().filter_map(|i| files.get(i)).collect()
}

/// Split `DSC03798` into (`DSC`, 3798). A name with no trailing digits gets number 0.
fn split_sequence(stem: &str) -> (String, u64) {
    let digits_start = stem
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    let (prefix, digits) = stem.split_at(digits_start);
    // More than 19 digits cannot be a frame counter and would overflow anyway.
    let number = if digits.is_empty() || digits.len() > 19 {
        0
    } else {
        digits.parse().unwrap_or(0)
    };
    (prefix.to_ascii_lowercase(), number)
}

fn read_header(path: &Path) -> Option<exif::Header> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; exif::HEADER_BYTES];
    let mut filled = 0usize;
    // A short read is normal at end-of-file; keep whatever we got.
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => break,
        }
    }
    buffer.truncate(filled);
    let header = exif::parse(&buffer);
    (!header.is_empty()).then_some(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(rel: &str, size: u64) -> IngestFile {
        IngestFile {
            rel_path: PathBuf::from(rel.replace('/', std::path::MAIN_SEPARATOR_STR)),
            src_abs: PathBuf::from("I:\\").join(rel),
            size,
            mtime: 0,
        }
    }

    fn names(files: &[IngestFile]) -> Vec<String> {
        files
            .iter()
            .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn a_jpeg_beside_its_raw_is_dropped() {
        let (kept, dropped) = drop_jpegs_shadowed_by_raw(vec![
            file("DCIM/101MSDCF/DSC03798.ARW", 10),
            file("DCIM/101MSDCF/DSC03798.JPG", 5),
            file("DCIM/101MSDCF/DSC03799.ARW", 10),
            file("DCIM/101MSDCF/DSC03799.JPG", 5),
        ]);
        assert_eq!(dropped, 2);
        assert_eq!(
            names(&kept),
            ["DCIM/101MSDCF/DSC03798.ARW", "DCIM/101MSDCF/DSC03799.ARW"]
        );
    }

    #[test]
    fn a_jpeg_with_no_raw_of_its_own_is_kept() {
        let (kept, dropped) = drop_jpegs_shadowed_by_raw(vec![
            file("DCIM/100/IMG_1.ARW", 10),
            file("DCIM/100/IMG_2.JPG", 5),
        ]);
        assert_eq!(dropped, 0);
        assert_eq!(names(&kept), ["DCIM/100/IMG_1.ARW", "DCIM/100/IMG_2.JPG"]);
    }

    #[test]
    fn pairing_never_reaches_across_directories() {
        // Two image folders can hold genuinely different shots under the same name, so a
        // RAW in one must not silently delete a JPEG in the other.
        let (kept, dropped) = drop_jpegs_shadowed_by_raw(vec![
            file("DCIM/115NCZ_7/PHC_2730.NEF", 10),
            file("DCIM/116NCZ_7/PHC_2730.JPG", 5),
        ]);
        assert_eq!(dropped, 0);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn pairing_ignores_case_the_way_the_filesystem_does() {
        let (kept, dropped) = drop_jpegs_shadowed_by_raw(vec![
            file("DCIM/100/img_1.nef", 10),
            file("DCIM/100/IMG_1.JPG", 5),
        ]);
        assert_eq!(dropped, 1);
        assert_eq!(names(&kept), ["DCIM/100/img_1.nef"]);
    }

    #[test]
    fn a_contiguous_run_needs_only_one_camera_sample() {
        // The Nikon card: PHC_2730..PHC_4849 with no gaps.
        let files: Vec<IngestFile> = (2730..=4849)
            .map(|n| file(&format!("DCIM/115NCZ_7/PHC_{n}.NEF"), 50))
            .collect();
        assert_eq!(names(&sample_files(&files).into_iter().cloned().collect::<Vec<_>>()).len(), 1);
    }

    #[test]
    fn a_gap_in_the_sequence_earns_a_second_sample() {
        // The Sony card: DSC03798..DSC03912 with one gap at 3854 -> 3857.
        let files: Vec<IngestFile> = (3798..=3912)
            .filter(|n| !(3855..=3856).contains(n))
            .map(|n| file(&format!("DCIM/101MSDCF/DSC0{n}.ARW"), 35))
            .collect();
        let samples = sample_files(&files);
        assert_eq!(samples.len(), 2);
        assert_eq!(
            names(&samples.into_iter().cloned().collect::<Vec<_>>()),
            ["DCIM/101MSDCF/DSC03798.ARW", "DCIM/101MSDCF/DSC03857.ARW"]
        );
    }

    #[test]
    fn different_folders_prefixes_and_formats_are_sampled_separately() {
        let files = vec![
            file("DCIM/100CANON/IMG_0001.CR2", 20),
            file("DCIM/100CANON/IMG_0002.CR2", 20),
            file("DCIM/101MSDCF/DSC00001.ARW", 30),
            file("DCIM/101MSDCF/PANO0001.ARW", 30),
            file("DCIM/101MSDCF/DSC00002.JPG", 5),
        ];
        assert_eq!(sample_files(&files).len(), 4);
    }

    #[test]
    fn sequence_splitting_handles_awkward_names() {
        assert_eq!(split_sequence("DSC03798"), ("dsc".into(), 3798));
        assert_eq!(split_sequence("PHC_2730"), ("phc_".into(), 2730));
        assert_eq!(split_sequence("IMG_0001"), ("img_".into(), 1));
        // No digits at all, and digits only.
        assert_eq!(split_sequence("PANORAMA"), ("panorama".into(), 0));
        assert_eq!(split_sequence("12345"), ("".into(), 12345));
        assert_eq!(split_sequence(""), ("".into(), 0));
        // Absurdly long digit runs must not overflow.
        let long = "9".repeat(40);
        assert_eq!(split_sequence(&long), ("".into(), 0));
    }

    #[test]
    fn skipped_counts_read_as_a_sentence() {
        assert_eq!(Skipped::default().describe(), None);
        assert_eq!(
            Skipped { jpeg_with_raw: 113, other: 0 }.describe().unwrap(),
            "skipped 113 JPEG with RAW"
        );
        assert_eq!(
            Skipped { jpeg_with_raw: 113, other: 47 }.describe().unwrap(),
            "skipped 113 JPEG with RAW, 47 non-image"
        );
    }

    /// Scan a real card: `PHASE_CARD_ROOT=I:\ cargo test -- --ignored --nocapture scans_a_real_card`
    #[test]
    #[ignore]
    fn scans_a_real_card() {
        let root = std::env::var("PHASE_CARD_ROOT").expect("set PHASE_CARD_ROOT");
        let scanned = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        let result = scan(Path::new(&root), &scanned);
        println!(
            "{root}: {} files, {}, cameras {:?}, {:.2} GB, {}, examined {} in {:.2}s",
            result.files.len(),
            result.describe_contents(),
            result.cameras,
            result.files.iter().map(|f| f.size).sum::<u64>() as f64 / 1_073_741_824.0,
            result.skipped.describe().unwrap_or_else(|| "skipped nothing".into()),
            scanned.load(Ordering::Relaxed),
            started.elapsed().as_secs_f32(),
        );
        for file in result.files.iter().take(2) {
            println!("  first: {}", file.rel_path.display());
        }
        if let Some(last) = result.files.last() {
            println!("  last:  {}", last.rel_path.display());
        }
        assert!(!result.files.is_empty());
    }

    #[test]
    fn scanning_a_real_directory_tree_picks_only_photographs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let write = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        };
        // Shaped after the real Canon-labelled card carrying Sony files.
        write("DCIM/101MSDCF/DSC03798.ARW");
        write("DCIM/101MSDCF/DSC03798.JPG");
        write("DCIM/101MSDCF/DSC03799.ARW");
        write("DCIM/101MSDCF/DSC03799.JPG");
        write("DCIM/EOSMISC/M100E001.CTG");
        write("ML/cropmks/CrssMtr2.bmp");
        write("ML/scripts/hello.lua");
        write("PRIVATE/AVCHD/BDMV/STREAM/00000.MTS");
        write("MP_ROOT/101ANV01/MAH00001.MP4");
        write("phase_log.txt");
        write("System Volume Information/IndexerVolumeGuid");

        let scanned = AtomicUsize::new(0);
        let result = scan(root, &scanned);

        assert_eq!(
            names(&result.files),
            ["DCIM/101MSDCF/DSC03798.ARW", "DCIM/101MSDCF/DSC03799.ARW"]
        );
        assert_eq!(result.skipped.jpeg_with_raw, 2);
        assert_eq!(result.ext_counts, vec![("ARW".to_string(), 2)]);
        assert_eq!(result.describe_contents(), "2 ARW");
        // Everything under System Volume Information is not even walked.
        assert!(scanned.load(Ordering::Relaxed) >= result.files.len());
    }
}
