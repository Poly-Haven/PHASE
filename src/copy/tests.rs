use super::plan::*;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

fn write(p: &std::path::Path, bytes: &[u8]) {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
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
    assert_eq!(
        classify(100, 1_000, Some(100), Some(1_000)),
        Action::Identical
    );
    assert_eq!(
        classify(100, 1_000, Some(100), Some(1_002)),
        Action::Identical
    );
}

#[test]
fn classify_overwrite_when_source_newer() {
    assert_eq!(
        classify(100, 2_000, Some(100), Some(1_000)),
        Action::Overwrite
    );
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
    assert_eq!(
        classify(100, 1_000, Some(101), Some(1_000)),
        Action::Overwrite
    );
}

#[test]
fn plan_includes_new_files_and_skips_pull_excluded() {
    let src = tempdir().unwrap();
    let dst = tempdir().unwrap();
    write(&src.path().join("a.exr"), b"hello");
    write(&src.path().join("sub/b.png"), b"world");
    write(&src.path().join("raw.NEF"), b"raw-bytes");
    write(&src.path().join("scan.tif"), b"tif-bytes");

    let plan = build_plan(Direction::Pull, src.path(), dst.path()).unwrap();
    let names: Vec<_> = plan
        .files
        .iter()
        .map(|f| f.rel_path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(names.contains(&"a.exr".to_string()));
    assert!(names.contains(&"sub/b.png".to_string()));
    assert!(!names.iter().any(|n| n.ends_with(".NEF")));
    assert!(!names.iter().any(|n| n.ends_with(".tif")));
    assert!(plan.files.iter().all(|f| matches!(f.action, Action::New)));
    assert_eq!(
        plan.total_bytes_to_copy,
        b"hello".len() as u64 + b"world".len() as u64
    );
}

#[test]
fn push_includes_tif_and_nef() {
    let src = tempdir().unwrap();
    let dst = tempdir().unwrap();
    write(&src.path().join("raw.NEF"), b"raw-bytes");
    write(&src.path().join("scan.tif"), b"tif-bytes");

    let plan = build_plan(Direction::Push, src.path(), dst.path()).unwrap();
    assert_eq!(plan.files.len(), 2);
}

#[test]
fn plan_ignores_partial_files() {
    let src = tempdir().unwrap();
    let dst = tempdir().unwrap();
    write(&src.path().join("a.exr.partial"), b"x");
    write(&src.path().join("a.exr"), b"y");

    let plan = build_plan(Direction::Push, src.path(), dst.path()).unwrap();
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].rel_path.to_string_lossy(), "a.exr");
}

use super::engine::{copy_one_file, CopyError};
use super::job::{self, JobMsg, JobProgress};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

#[test]
fn copy_one_file_writes_and_validates() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("nested/dst.bin");
    let payload: Vec<u8> = (0..(9 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();
    write(&src, &payload);

    let bytes = AtomicU64::new(0);
    let cancel = AtomicBool::new(false);

    copy_one_file(&src, &dst, &bytes, &cancel).unwrap();

    let on_disk = std::fs::read(&dst).unwrap();
    assert_eq!(on_disk, payload);
    assert_eq!(bytes.load(Ordering::Relaxed), payload.len() as u64);
    let partial = dst.with_file_name("dst.bin.partial");
    assert!(!partial.exists());
}

#[test]
fn copy_one_file_cancel_removes_partial_and_leaves_dest_untouched() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.bin");
    let payload: Vec<u8> = vec![7u8; 32 * 1024 * 1024];
    write(&src, &payload);
    write(&dst, b"original");

    let bytes = AtomicU64::new(0);
    let cancel = AtomicBool::new(true);
    let err = copy_one_file(&src, &dst, &bytes, &cancel).unwrap_err();
    assert!(matches!(err, CopyError::Cancelled));

    assert_eq!(std::fs::read(&dst).unwrap(), b"original");
    let partial = dst.with_file_name("dst.bin.partial");
    assert!(!partial.exists(), "partial should be cleaned up");
}

#[test]
fn copy_one_file_preserves_source_mtime() {
    use filetime::{set_file_mtime, FileTime};
    let dir = tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.bin");
    write(&src, b"hi");
    let stamp = FileTime::from_unix_time(1_700_000_000, 0);
    set_file_mtime(&src, stamp).unwrap();

    let bytes = AtomicU64::new(0);
    let cancel = AtomicBool::new(false);
    copy_one_file(&src, &dst, &bytes, &cancel).unwrap();

    let md = std::fs::metadata(&dst).unwrap();
    assert_eq!(
        FileTime::from_last_modification_time(&md).unix_seconds(),
        1_700_000_000
    );
}

#[test]
fn copy_worker_count_is_capped_and_never_exceeds_copyable_files() {
    assert_eq!(job::copy_worker_count(0), 1);
    assert_eq!(job::copy_worker_count(1), 1);
    assert_eq!(job::copy_worker_count(2), 2);
    assert_eq!(job::copy_worker_count(20), 4);
}

#[test]
#[ignore = "copies 5 GiB of dummy data; run explicitly with --ignored --nocapture"]
fn benchmark_job_copy_5gib_pull_and_push() {
    let total_bytes = std::env::var("PHASE_COPY_BENCH_GIB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5)
        * 1024
        * 1024
        * 1024;
    let file_count = std::env::var("PHASE_COPY_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .max(1);

    let cfg = crate::config::load().unwrap_or_default();
    let prod_base = std::env::var_os("PHASE_COPY_BENCH_PROD_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cfg.prod_root.join("HDRIs").join("aarfontein_dirt_road"));
    let local_base = std::env::var_os("PHASE_COPY_BENCH_LOCAL_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cfg.local_root.join("HDRIs").join("aarfontein_dirt_road"));

    let run_id = format!("phase-copy-bench-{}", std::process::id());
    let prod_bench_root = prod_base.join("__phase_copy_bench").join(&run_id);
    let local_bench_root = local_base.join("__phase_copy_bench").join(&run_id);
    let prod_src = prod_bench_root.join("prod-source");
    let local_dst = local_bench_root.join("local-dest");
    let prod_dst = prod_bench_root.join("prod-dest");

    let _ = fs::remove_dir_all(&prod_bench_root);
    let _ = fs::remove_dir_all(&local_bench_root);
    let _cleanup = BenchCleanup {
        prod_bench_root: prod_bench_root.clone(),
        local_bench_root: local_bench_root.clone(),
    };

    write_dummy_dataset(&prod_src, total_bytes, file_count);

    let worker_count = std::env::var("PHASE_COPY_BENCH_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());

    let pull = benchmark_job_copy(Direction::Pull, &prod_src, &local_dst, worker_count);
    println!(
        "pull: copied {:.2} GiB in {:.2}s ({:.1} MiB/s, workers={})",
        bytes_to_gib(pull.bytes),
        pull.seconds,
        bytes_to_mib(pull.bytes) / pull.seconds,
        worker_count.unwrap_or_else(|| job::copy_worker_count(file_count))
    );

    let push = benchmark_job_copy(Direction::Push, &local_dst, &prod_dst, worker_count);
    println!(
        "push: copied {:.2} GiB in {:.2}s ({:.1} MiB/s, workers={})",
        bytes_to_gib(push.bytes),
        push.seconds,
        bytes_to_mib(push.bytes) / push.seconds,
        worker_count.unwrap_or_else(|| job::copy_worker_count(file_count))
    );

}

struct BenchCleanup {
    prod_bench_root: std::path::PathBuf,
    local_bench_root: std::path::PathBuf,
}

impl Drop for BenchCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.prod_bench_root);
        let _ = fs::remove_dir_all(&self.local_bench_root);
    }
}

struct BenchResult {
    bytes: u64,
    seconds: f64,
}

fn benchmark_job_copy(
    direction: Direction,
    src: &std::path::Path,
    dst: &std::path::Path,
    worker_count: Option<usize>,
) -> BenchResult {
    let plan = build_plan(direction, src, dst).unwrap();
    let bytes = plan.total_bytes_to_copy;
    let progress = Arc::new(JobProgress::default());
    let (tx, rx) = mpsc::channel();

    let started = Instant::now();
    let handle = job::spawn_with_worker_count(plan, progress.clone(), tx, worker_count);
    let mut finished = false;
    while let Ok(msg) = rx.recv() {
        match msg {
            JobMsg::Finished => {
                finished = true;
                break;
            }
            JobMsg::Cancelled => panic!("benchmark copy was cancelled"),
            JobMsg::FileFailed { rel_path, error } => {
                panic!("benchmark copy failed for {rel_path}: {error}");
            }
            JobMsg::FileDone { .. } => {}
        }
    }
    handle.join().unwrap();

    assert!(finished, "benchmark copy worker exited without Finished");
    assert_eq!(progress.bytes_done.load(Ordering::Relaxed), bytes);

    BenchResult {
        bytes,
        seconds: started.elapsed().as_secs_f64(),
    }
}

fn write_dummy_dataset(root: &std::path::Path, total_bytes: u64, file_count: usize) {
    fs::create_dir_all(root).unwrap();
    let mut pattern = vec![0u8; 8 * 1024 * 1024];
    for (i, byte) in pattern.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }

    let base_size = total_bytes / file_count as u64;
    let remainder = total_bytes % file_count as u64;
    for index in 0..file_count {
        let extra = if index == file_count - 1 {
            remainder
        } else {
            0
        };
        let size = base_size + extra;
        let path = root.join(format!("dummy_{index:03}.bin"));
        let mut file = fs::File::create(path).unwrap();
        let mut remaining = size;
        while remaining > 0 {
            let n = remaining.min(pattern.len() as u64) as usize;
            file.write_all(&pattern[..n]).unwrap();
            remaining -= n as u64;
        }
        file.flush().unwrap();
    }
}

fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0 / 1024.0
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}
