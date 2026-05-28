use super::plan::*;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

fn write(p: &std::path::Path, bytes: &[u8]) {
    if let Some(parent) = p.parent() { fs::create_dir_all(parent).unwrap(); }
    let mut f = fs::File::create(p).unwrap();
    f.write_all(bytes).unwrap();
}

#[test]
fn pull_excludes_tif_tiff_nef_case_insensitive() {
    assert!(is_excluded_for_pull("foo.tif"));
    assert!(is_excluded_for_pull("foo.TIF"));
    assert!(is_excluded_for_pull("foo.tiff"));
    assert!(is_excluded_for_pull("foo.NEF"));
    assert!(is_excluded_for_pull("a.b.nef"));
    assert!(!is_excluded_for_pull("foo.exr"));
    assert!(!is_excluded_for_pull("foo.png"));
    assert!(!is_excluded_for_pull("NEF"));
    assert!(!is_excluded_for_pull(".tif"));
}

#[test]
fn classify_new_when_dst_missing() {
    assert_eq!(classify(100, 1_000, None, None), Action::New);
}

#[test]
fn classify_identical_when_size_and_mtime_match() {
    assert_eq!(classify(100, 1_000, Some(100), Some(1_000)), Action::Identical);
    assert_eq!(classify(100, 1_000, Some(100), Some(1_002)), Action::Identical);
}

#[test]
fn classify_overwrite_when_source_newer() {
    assert_eq!(classify(100, 2_000, Some(100), Some(1_000)), Action::Overwrite);
}

#[test]
fn classify_conflict_when_dest_newer() {
    assert_eq!(
        classify(100, 1_000, Some(100), Some(2_000)),
        Action::Conflict { dest_newer: true }
    );
}

#[test]
fn classify_overwrite_when_sizes_differ_within_mtime_tolerance() {
    assert_eq!(classify(100, 1_000, Some(101), Some(1_000)), Action::Overwrite);
}

#[test]
fn plan_includes_new_files_and_skips_pull_excluded() {
    let src = tempdir().unwrap();
    let dst = tempdir().unwrap();
    write(&src.path().join("a.exr"),     b"hello");
    write(&src.path().join("sub/b.png"), b"world");
    write(&src.path().join("raw.NEF"),   b"raw-bytes");
    write(&src.path().join("scan.tif"),  b"tif-bytes");

    let plan = build_plan(Direction::Pull, src.path(), dst.path()).unwrap();
    let names: Vec<_> = plan.files.iter()
        .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(names.contains(&"a.exr".to_string()));
    assert!(names.contains(&"sub/b.png".to_string()));
    assert!(!names.iter().any(|n| n.ends_with(".NEF")));
    assert!(!names.iter().any(|n| n.ends_with(".tif")));
    assert!(plan.files.iter().all(|f| matches!(f.action, Action::New)));
    assert_eq!(plan.total_bytes_to_copy, b"hello".len() as u64 + b"world".len() as u64);
}

#[test]
fn push_includes_tif_and_nef() {
    let src = tempdir().unwrap();
    let dst = tempdir().unwrap();
    write(&src.path().join("raw.NEF"),  b"raw-bytes");
    write(&src.path().join("scan.tif"), b"tif-bytes");

    let plan = build_plan(Direction::Push, src.path(), dst.path()).unwrap();
    assert_eq!(plan.files.len(), 2);
}

#[test]
fn plan_ignores_partial_files() {
    let src = tempdir().unwrap();
    let dst = tempdir().unwrap();
    write(&src.path().join("a.exr.partial"), b"x");
    write(&src.path().join("a.exr"),         b"y");

    let plan = build_plan(Direction::Push, src.path(), dst.path()).unwrap();
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].rel_path.to_string_lossy(), "a.exr");
}
