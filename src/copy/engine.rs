use anyhow::Context;
use filetime::{set_file_mtime, FileTime};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const CHUNK: usize = 8 * 1024 * 1024; // 8 MiB

#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("cancelled")]
    Cancelled,
    #[error("hash mismatch after copy ({path})")]
    HashMismatch { path: PathBuf },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(
        "size mismatch after copy: source is {src_size} bytes, destination is {dst_size} bytes"
    )]
    SizeMismatch { src_size: u64, dst_size: u64 },
    #[error("mtime mismatch after copy: source is {src_mtime}, destination is {dst_mtime}")]
    MtimeMismatch { src_mtime: i64, dst_mtime: i64 },
    #[error("hash mismatch after copy")]
    HashMismatch,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Stream-copy `src` → `dst` with BLAKE3 validation, atomic rename, mtime preservation.
///
/// `bytes_done` is incremented by every chunk written. `cancel` is checked between chunks;
/// on cancel the `.partial` is removed and the existing destination (if any) is untouched.
pub fn copy_one_file(
    src: &Path,
    dst: &Path,
    bytes_done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<(), CopyError> {
    copy_one_file_inner(src, dst, bytes_done, cancel, true)
}

pub fn copy_one_file_deferred_verify(
    src: &Path,
    dst: &Path,
    bytes_done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<(), CopyError> {
    copy_one_file_inner(src, dst, bytes_done, cancel, false)
}

fn copy_one_file_inner(
    src: &Path,
    dst: &Path,
    bytes_done: &AtomicU64,
    cancel: &AtomicBool,
    verify_before_rename: bool,
) -> Result<(), CopyError> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }

    let partial = partial_path(dst);
    let _ = fs::remove_file(&partial);

    let mut hasher = verify_before_rename.then(blake3::Hasher::new);

    {
        let mut src_f =
            fs::File::open(src).with_context(|| format!("open src {}", src.display()))?;
        let mut dst_f = fs::File::create(&partial)
            .with_context(|| format!("create partial {}", partial.display()))?;

        let mut buf = vec![0u8; CHUNK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                drop(dst_f);
                let _ = fs::remove_file(&partial);
                return Err(CopyError::Cancelled);
            }
            let n = src_f
                .read(&mut buf)
                .with_context(|| format!("read {}", src.display()))?;
            if n == 0 {
                break;
            }
            if let Some(hasher) = &mut hasher {
                hasher.update(&buf[..n]);
            }
            dst_f
                .write_all(&buf[..n])
                .with_context(|| format!("write {}", partial.display()))?;
            bytes_done.fetch_add(n as u64, Ordering::Relaxed);
        }

        dst_f
            .flush()
            .with_context(|| format!("flush {}", partial.display()))?;
        dst_f
            .sync_all()
            .with_context(|| format!("fsync {}", partial.display()))?;
    }

    if verify_before_rename {
        let src_hash = hasher
            .expect("copy hash must exist when immediate verification is enabled")
            .finalize();
        let dst_hash =
            hash_file(&partial).with_context(|| format!("validate-read {}", partial.display()))?;

        if src_hash != dst_hash {
            let _ = fs::remove_file(&partial);
            return Err(CopyError::HashMismatch {
                path: dst.to_path_buf(),
            });
        }
    }

    rename_replacing(&partial, dst)
        .with_context(|| format!("rename {} -> {}", partial.display(), dst.display()))?;

    let src_md = fs::metadata(src).with_context(|| format!("stat {}", src.display()))?;
    set_file_mtime(dst, FileTime::from_last_modification_time(&src_md))
        .with_context(|| format!("set mtime {}", dst.display()))?;

    Ok(())
}

pub fn verify_copied_file(src: &Path, dst: &Path) -> Result<(), VerifyError> {
    let src_md = fs::metadata(src).with_context(|| format!("stat {}", src.display()))?;
    let dst_md = fs::metadata(dst).with_context(|| format!("stat {}", dst.display()))?;
    let src_size = src_md.len();
    let dst_size = dst_md.len();
    if src_size != dst_size {
        return Err(VerifyError::SizeMismatch { src_size, dst_size });
    }

    let src_mtime = FileTime::from_last_modification_time(&src_md).unix_seconds();
    let dst_mtime = FileTime::from_last_modification_time(&dst_md).unix_seconds();
    if (src_mtime - dst_mtime).abs() > 2 {
        return Err(VerifyError::MtimeMismatch {
            src_mtime,
            dst_mtime,
        });
    }

    let src_hash = hash_file(src).with_context(|| format!("hash {}", src.display()))?;
    let dst_hash = hash_file(dst).with_context(|| format!("hash {}", dst.display()))?;
    if src_hash != dst_hash {
        return Err(VerifyError::HashMismatch);
    }

    Ok(())
}

fn partial_path(dst: &Path) -> PathBuf {
    let mut s = dst.as_os_str().to_owned();
    s.push(".partial");
    PathBuf::from(s)
}

fn hash_file(path: &Path) -> anyhow::Result<blake3::Hash> {
    let mut f = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize())
}

/// `fs::rename` on Windows fails if the destination exists; remove first.
fn rename_replacing(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        fs::remove_file(to)?;
    }
    fs::rename(from, to)
}
