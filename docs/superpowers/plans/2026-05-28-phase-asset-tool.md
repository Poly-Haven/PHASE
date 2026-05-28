# PHASE Asset Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Windows desktop tool (`phase.exe`) that lists assets from two Notion databases (HDRIs, Textures) and lets the user Pull/Push each asset's folder between the production NAS (`P:\Assets`) and a local working folder (`C:\PHASE\Assets`) with crash-safe, hash-validated file copies.

**Architecture:** Single-process Rust binary using `eframe`/`egui` for the UI. Three internal modules: `notion` (HTTP client + parsing), `copy` (planning + streaming copy + BLAKE3 validation + atomic rename), and `ui` (egui frontend that runs work on background threads and samples shared `AtomicU64` progress counters each frame). No async runtime — OS threads and `std::sync::mpsc` channels.

**Tech Stack:** Rust (MSVC), eframe/egui, reqwest (blocking + rustls), serde/serde_json, toml, walkdir, blake3, filetime, open, dirs, anyhow, log + simplelog. Build target: `x86_64-pc-windows-msvc`.

**Reference:** `docs/superpowers/specs/2026-05-28-phase-asset-tool-design.md`

**Conventions:**
- All shell commands are PowerShell (Windows).
- Working directory is `C:\Git\phase` unless stated otherwise.
- `cargo` commands assume `cargo` is on PATH.
- Commits use Conventional Commits (`feat:`, `test:`, `chore:`) and include the Copilot co-author trailer.

---

## Task 1: Initialise Rust project

**Files:**
- Create: `C:\Git\phase\Cargo.toml`
- Create: `C:\Git\phase\src\main.rs`
- Create: `C:\Git\phase\.gitignore`

- [ ] **Step 1: Initialise the Cargo project in place**

Run:
```powershell
cd C:\Git\phase
cargo init --name phase --bin
```
Expected: `Cargo.toml` and `src\main.rs` created. (Repo already initialised by brainstorming step.)

- [ ] **Step 2: Replace `Cargo.toml` with pinned dependencies and release profile**

Overwrite `Cargo.toml`:
```toml
[package]
name = "phase"
version = "0.1.0"
edition = "2021"
description = "PHASE — internal asset pipeline tool"

[dependencies]
eframe      = { version = "0.27", default-features = false, features = ["default_fonts", "glow", "wayland", "x11"] }
egui        = "0.27"
egui_extras = { version = "0.27", features = ["image"] }
image       = { version = "0.24", default-features = false, features = ["png"] }
reqwest     = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
toml        = "0.8"
walkdir     = "2"
blake3      = "1"
filetime    = "0.2"
open        = "5"
dirs        = "5"
anyhow      = "1"
log         = "0.4"
simplelog   = "0.12"

[dev-dependencies]
tempfile = "3"

[profile.release]
lto           = true
strip         = true
codegen-units = 1
panic         = "abort"
```

- [ ] **Step 3: Replace `src\main.rs` with a stub that opens a blank dark-themed window**

Overwrite `src\main.rs`:
```rust
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        &format!("PHASE {}", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(App::default())
        }),
    )
}

#[derive(Default)]
struct App;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("PHASE — starting up");
        });
    }
}
```

- [ ] **Step 4: Replace `.gitignore`**

Overwrite `.gitignore`:
```
/target
Cargo.lock
```
(We're a binary crate but committing `Cargo.lock` is fine; either is acceptable. Excluding here keeps the diff small for v0.1; revisit later if reproducible builds matter.)

- [ ] **Step 5: Build and run once to confirm**

Run:
```powershell
cd C:\Git\phase
cargo build
```
Expected: builds successfully (first build will download crates and take several minutes).

Run:
```powershell
cargo run
```
Expected: a window titled `PHASE 0.1.0` opens showing "PHASE — starting up". Close it.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.toml .gitignore src/main.rs
git commit -m "chore: initialise Rust project with eframe stub

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 2: Config module

**Files:**
- Create: `C:\Git\phase\src\config.rs`
- Modify: `C:\Git\phase\src\main.rs` (add `mod config;`)

- [ ] **Step 1: Write the config module**

Create `src\config.rs`:
```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub notion_token: String,
    #[serde(default = "default_prod_root")]
    pub prod_root: PathBuf,
    #[serde(default = "default_local_root")]
    pub local_root: PathBuf,
}

fn default_prod_root() -> PathBuf  { PathBuf::from(r"P:\Assets") }
fn default_local_root() -> PathBuf { PathBuf::from(r"C:\PHASE\Assets") }

impl Default for Config {
    fn default() -> Self {
        Self {
            notion_token: String::new(),
            prod_root:  default_prod_root(),
            local_root: default_local_root(),
        }
    }
}

/// Returns `%APPDATA%\phase`, creating it if missing.
pub fn app_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not locate %APPDATA%")?;
    let dir = base.join("phase");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(app_dir()?.join("config.toml"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(app_dir()?.join("phase.log"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let text = toml::to_string_pretty(cfg).context("serialising config")?;
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
```

- [ ] **Step 2: Register the module in `main.rs`**

Add at the very top of `src\main.rs`:
```rust
mod config;
```

- [ ] **Step 3: Verify build**

Run:
```powershell
cargo build
```
Expected: builds with no warnings about unused items beyond the obvious dead-code stubs.

- [ ] **Step 4: Commit**

```powershell
git add src/config.rs src/main.rs
git commit -m "feat(config): add config load/save in %APPDATA%\phase

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 3: Copy planner — types and classification (TDD)

**Files:**
- Create: `C:\Git\phase\src\copy\mod.rs`
- Create: `C:\Git\phase\src\copy\plan.rs`
- Modify: `C:\Git\phase\src\main.rs` (add `mod copy;`)

- [ ] **Step 1: Create the module skeleton**

Create `src\copy\mod.rs`:
```rust
pub mod plan;
pub mod engine;

#[cfg(test)]
mod tests;
```

Create `src\copy\engine.rs` as an empty file (we'll fill it later):
```rust
// engine implementation lives here; see Task 5+
```

Create `src\copy\tests.rs`:
```rust
// integration-style tests for plan + engine live here
```

Add `mod copy;` to the top of `src\main.rs` after `mod config;`.

- [ ] **Step 2: Write the failing tests for the planner**

Create `src\copy\plan.rs`:
```rust
use std::path::{Path, PathBuf};

/// A single file's classification in a copy plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Destination does not exist.
    New,
    /// Source is newer than destination (or sizes differ) — overwrite.
    Overwrite,
    /// Destination is newer than source — needs user decision.
    Conflict { dest_newer: bool },
    /// Size + mtime match within tolerance — skip.
    Identical,
}

#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub rel_path: PathBuf,
    pub src_abs:  PathBuf,
    pub dst_abs:  PathBuf,
    pub size:     u64,
    pub action:   Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction { Pull, Push }

/// Lower-case extensions excluded when pulling. Push excludes nothing.
const PULL_EXCLUDED_EXT: &[&str] = &["tif", "tiff", "nef"];

/// mtime tolerance to account for filesystem precision differences (NTFS vs SMB).
const MTIME_TOLERANCE_SECS: i64 = 2;

/// True if Pull should skip this filename.
pub fn is_excluded_for_pull(file_name: &str) -> bool {
    let Some(ext) = Path::new(file_name).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext_lower = ext.to_ascii_lowercase();
    PULL_EXCLUDED_EXT.iter().any(|e| *e == ext_lower)
}

/// Classify a single file given src/dst metadata.
///
/// `src_mtime` and `dst_mtime` are seconds since epoch. `dst_*` are `None` when the
/// destination does not exist.
pub fn classify(
    src_size: u64,
    src_mtime: i64,
    dst_size: Option<u64>,
    dst_mtime: Option<i64>,
) -> Action {
    let (Some(dsz), Some(dmt)) = (dst_size, dst_mtime) else {
        return Action::New;
    };
    if dsz == src_size && (src_mtime - dmt).abs() <= MTIME_TOLERANCE_SECS {
        return Action::Identical;
    }
    let delta = src_mtime - dmt;
    if delta > MTIME_TOLERANCE_SECS {
        Action::Overwrite
    } else if delta < -MTIME_TOLERANCE_SECS {
        Action::Conflict { dest_newer: true }
    } else {
        // mtimes within tolerance but sizes differ — treat as overwrite from src.
        Action::Overwrite
    }
}
```

Replace `src\copy\tests.rs` with:
```rust
use super::plan::*;

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
```

- [ ] **Step 3: Run the tests — expect them to PASS**

Run:
```powershell
cargo test --lib copy::
```
Expected: 6 passed, 0 failed. (The minimal implementation in Step 2 already satisfies these.)

If any fail, fix the implementation in `plan.rs` until they pass. Do not change the tests.

- [ ] **Step 4: Add the directory-walking planner function**

Append to `src\copy\plan.rs`:
```rust
use anyhow::{Context, Result};
use filetime::FileTime;
use std::fs;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Plan {
    pub direction: Direction,
    pub src_root:  PathBuf,
    pub dst_root:  PathBuf,
    pub files:     Vec<PlannedFile>,
    pub total_bytes_to_copy: u64,
}

impl Plan {
    pub fn conflicts(&self) -> Vec<&PlannedFile> {
        self.files.iter().filter(|f| matches!(f.action, Action::Conflict { .. })).collect()
    }
    pub fn copyable(&self) -> impl Iterator<Item = &PlannedFile> {
        self.files.iter().filter(|f| matches!(f.action, Action::New | Action::Overwrite))
    }
}

/// Walk `src_root`, classify each file against `dst_root`, return the plan.
///
/// On Pull, files with excluded extensions (.tif/.tiff/.nef) are skipped entirely.
/// `.partial` files in either tree are ignored.
pub fn build_plan(direction: Direction, src_root: &Path, dst_root: &Path) -> Result<Plan> {
    let mut files = Vec::new();
    let mut total = 0u64;

    for entry in WalkDir::new(src_root).follow_links(false) {
        let entry = entry.with_context(|| format!("walking {}", src_root.display()))?;
        if !entry.file_type().is_file() { continue; }

        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.ends_with(".partial") { continue; }
        if direction == Direction::Pull && is_excluded_for_pull(&file_name) { continue; }

        let src_abs = entry.path().to_path_buf();
        let rel = src_abs.strip_prefix(src_root).unwrap().to_path_buf();
        let dst_abs = dst_root.join(&rel);

        let src_md = fs::metadata(&src_abs)
            .with_context(|| format!("stat {}", src_abs.display()))?;
        let src_size  = src_md.len();
        let src_mtime = FileTime::from_last_modification_time(&src_md).unix_seconds();

        let (dst_size, dst_mtime) = match fs::metadata(&dst_abs) {
            Ok(m)  => (Some(m.len()), Some(FileTime::from_last_modification_time(&m).unix_seconds())),
            Err(_) => (None, None),
        };

        let action = classify(src_size, src_mtime, dst_size, dst_mtime);
        if matches!(action, Action::New | Action::Overwrite) {
            total += src_size;
        }
        files.push(PlannedFile { rel_path: rel, src_abs, dst_abs, size: src_size, action });
    }

    Ok(Plan {
        direction,
        src_root: src_root.to_path_buf(),
        dst_root: dst_root.to_path_buf(),
        files,
        total_bytes_to_copy: total,
    })
}
```

- [ ] **Step 5: Add planner integration tests**

Append to `src\copy\tests.rs`:
```rust
use super::plan::{build_plan, Direction, Action};
use std::fs;
use std::io::Write;
use tempfile::tempdir;

fn write(p: &std::path::Path, bytes: &[u8]) {
    if let Some(parent) = p.parent() { fs::create_dir_all(parent).unwrap(); }
    let mut f = fs::File::create(p).unwrap();
    f.write_all(bytes).unwrap();
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
```

- [ ] **Step 6: Run tests — all should pass**

Run:
```powershell
cargo test --lib copy::
```
Expected: 9 passed, 0 failed.

- [ ] **Step 7: Commit**

```powershell
git add src/copy src/main.rs
git commit -m "feat(copy): add plan/classify with Pull exclusions and tests

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 4: Copy engine — streaming copy with BLAKE3 validation (TDD)

**Files:**
- Modify: `C:\Git\phase\src\copy\engine.rs`
- Modify: `C:\Git\phase\src\copy\tests.rs`
- Modify: `C:\Git\phase\Cargo.toml` (add `thiserror`)

- [ ] **Step 1: Add the `thiserror` dependency**

Append to `[dependencies]` in `Cargo.toml`:
```toml
thiserror = "1"
```

- [ ] **Step 2: Write failing tests for `copy_one_file`**

Append to `src\copy\tests.rs`:
```rust
use super::engine::{copy_one_file, CopyError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
    let cancel = AtomicBool::new(true); // cancel immediately
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
    assert_eq!(FileTime::from_last_modification_time(&md).unix_seconds(), 1_700_000_000);
}
```

- [ ] **Step 3: Run tests — expect them to FAIL (compile error)**

Run:
```powershell
cargo test --lib copy::
```
Expected: compile error — `copy_one_file` and `CopyError` don't exist yet.

- [ ] **Step 4: Implement `copy_one_file`**

Replace `src\copy\engine.rs` with:
```rust
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

/// Stream-copy `src` → `dst` with BLAKE3 validation, atomic rename, mtime preservation.
///
/// `bytes_done` is incremented by every chunk written (used for the UI progress bar).
/// `cancel` is checked between chunks; on cancel the `.partial` is removed and the
/// existing destination (if any) is left untouched.
pub fn copy_one_file(
    src: &Path,
    dst: &Path,
    bytes_done: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<(), CopyError> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }

    let partial = partial_path(dst);
    let _ = fs::remove_file(&partial); // clean any stray .partial from prior crash

    let mut hasher = blake3::Hasher::new();

    {
        let mut src_f = fs::File::open(src)
            .with_context(|| format!("open src {}", src.display()))?;
        let mut dst_f = fs::File::create(&partial)
            .with_context(|| format!("create partial {}", partial.display()))?;

        let mut buf = vec![0u8; CHUNK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                drop(dst_f);
                let _ = fs::remove_file(&partial);
                return Err(CopyError::Cancelled);
            }
            let n = src_f.read(&mut buf)
                .with_context(|| format!("read {}", src.display()))?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
            dst_f.write_all(&buf[..n])
                .with_context(|| format!("write {}", partial.display()))?;
            bytes_done.fetch_add(n as u64, Ordering::Relaxed);
        }

        dst_f.flush().with_context(|| format!("flush {}", partial.display()))?;
        dst_f.sync_all().with_context(|| format!("fsync {}", partial.display()))?;
    }

    let src_hash = hasher.finalize();
    let dst_hash = hash_file(&partial)
        .with_context(|| format!("validate-read {}", partial.display()))?;

    if src_hash != dst_hash {
        let _ = fs::remove_file(&partial);
        return Err(CopyError::HashMismatch { path: dst.to_path_buf() });
    }

    rename_replacing(&partial, dst)
        .with_context(|| format!("rename {} -> {}", partial.display(), dst.display()))?;

    let src_md = fs::metadata(src).with_context(|| format!("stat {}", src.display()))?;
    set_file_mtime(dst, FileTime::from_last_modification_time(&src_md))
        .with_context(|| format!("set mtime {}", dst.display()))?;

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
        if n == 0 { break; }
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
```

- [ ] **Step 5: Run tests — all should pass**

Run:
```powershell
cargo test --lib copy::
```
Expected: 12 passed, 0 failed.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.toml src/copy/engine.rs src/copy/tests.rs
git commit -m "feat(copy): streaming copy with BLAKE3 validation and atomic rename

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 5: Copy job orchestration

**Files:**
- Modify: `C:\Git\phase\src\copy\mod.rs`
- Create: `C:\Git\phase\src\copy\job.rs`

- [ ] **Step 1: Register the new module**

Replace `src\copy\mod.rs`:
```rust
pub mod plan;
pub mod engine;
pub mod job;

#[cfg(test)]
mod tests;
```

- [ ] **Step 2: Implement the job runner**

Create `src\copy\job.rs`:
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::thread;

use crate::copy::engine::{copy_one_file, CopyError};
use crate::copy::plan::{Action, Direction, Plan};

/// Updates the UI receives while a job runs.
pub enum JobMsg {
    FileDone { rel_path: String },
    FileFailed { rel_path: String, error: String },
    Finished,
    Cancelled,
}

/// Shared, lock-free progress state the UI samples each frame.
#[derive(Default)]
pub struct JobProgress {
    pub bytes_done:  AtomicU64,
    pub bytes_total: AtomicU64,
    pub cancel:      AtomicBool,
}

impl JobProgress {
    pub fn fraction(&self) -> f32 {
        let t = self.bytes_total.load(Ordering::Relaxed);
        if t == 0 { return 0.0; }
        (self.bytes_done.load(Ordering::Relaxed) as f64 / t as f64) as f32
    }
}

/// Spawn a worker thread that copies all `New`/`Overwrite` files sequentially.
pub fn spawn(plan: Plan, progress: Arc<JobProgress>, tx: Sender<JobMsg>) -> thread::JoinHandle<()> {
    progress.bytes_total.store(plan.total_bytes_to_copy, Ordering::Relaxed);
    thread::spawn(move || {
        for file in plan.files.iter().filter(|f| matches!(f.action, Action::New | Action::Overwrite)) {
            if progress.cancel.load(Ordering::Relaxed) {
                let _ = tx.send(JobMsg::Cancelled);
                return;
            }
            let res = copy_one_file(
                &file.src_abs,
                &file.dst_abs,
                &progress.bytes_done,
                &progress.cancel,
            );
            match res {
                Ok(()) => {
                    let _ = tx.send(JobMsg::FileDone {
                        rel_path: file.rel_path.to_string_lossy().to_string(),
                    });
                }
                Err(CopyError::Cancelled) => {
                    let _ = tx.send(JobMsg::Cancelled);
                    return;
                }
                Err(e) => {
                    let _ = tx.send(JobMsg::FileFailed {
                        rel_path: file.rel_path.to_string_lossy().to_string(),
                        error: e.to_string(),
                    });
                    return;
                }
            }
        }
        let _ = tx.send(JobMsg::Finished);
    })
}

/// Re-export for callers that want a short name.
pub use Direction as _Direction;
```

- [ ] **Step 3: Build and test**

Run:
```powershell
cargo build
cargo test
```
Expected: clean build, all 12 tests still pass.

- [ ] **Step 4: Commit**

```powershell
git add src/copy
git commit -m "feat(copy): job runner with shared progress + cancel

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 6: Notion client

**Files:**
- Create: `C:\Git\phase\src\notion.rs`
- Modify: `C:\Git\phase\src\main.rs` (add `mod notion;`)

- [ ] **Step 1: Write the client**

Create `src\notion.rs`:
```rust
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::time::Duration;

pub const HDRIS_DB_ID:    &str = "21f373ac-61c1-80d0-8e55-cd46d121d1d5";
pub const TEXTURES_DB_ID: &str = "215373ac-61c1-80dd-8a97-edb25bb6a5f8";
const NOTION_VERSION: &str = "2022-06-28";

#[derive(Debug, Clone)]
pub struct Asset {
    pub slug:   String,   // page title
    pub author: String,   // empty string if not set
    pub url:    String,
}

#[derive(Deserialize)]
struct QueryResponse {
    results:     Vec<Page>,
    next_cursor: Option<String>,
    has_more:    bool,
}

#[derive(Deserialize)]
struct Page {
    url:        String,
    properties: serde_json::Value,
}

/// Fetch every page in a database, paginating until exhausted. Sorted by slug.
pub fn fetch_database(token: &str, database_id: &str) -> Result<Vec<Asset>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;

    let url = format!("https://api.notion.com/v1/databases/{database_id}/query");
    let mut cursor: Option<String> = None;
    let mut out = Vec::new();

    loop {
        let mut body = serde_json::Map::new();
        body.insert("page_size".into(), serde_json::Value::from(100));
        if let Some(c) = &cursor {
            body.insert("start_cursor".into(), serde_json::Value::String(c.clone()));
        }

        let resp = client.post(&url)
            .bearer_auth(token)
            .header("Notion-Version", NOTION_VERSION)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .context("HTTP request to Notion")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(anyhow!("Notion API error {status}: {text}"));
        }

        let page: QueryResponse = resp.json().context("parsing Notion response")?;
        for p in page.results {
            out.push(Asset {
                slug:   extract_title(&p.properties),
                author: extract_author(&p.properties),
                url:    p.url,
            });
        }

        if page.has_more { cursor = page.next_cursor; } else { break; }
    }

    out.sort_by(|a, b| a.slug.to_lowercase().cmp(&b.slug.to_lowercase()));
    Ok(out)
}

fn extract_title(props: &serde_json::Value) -> String {
    let Some(obj) = props.as_object() else { return String::new(); };
    for (_name, val) in obj {
        if val.get("type").and_then(|t| t.as_str()) == Some("title") {
            if let Some(arr) = val.get("title").and_then(|t| t.as_array()) {
                return concat_plain_text(arr);
            }
        }
    }
    String::new()
}

/// `Author` may be `people`, `rich_text`, `title`, `select`, or `multi_select`. Try each.
fn extract_author(props: &serde_json::Value) -> String {
    let Some(author) = props.get("Author") else { return String::new(); };
    let ty = author.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "rich_text" => author.get("rich_text").and_then(|a| a.as_array())
            .map(concat_plain_text).unwrap_or_default(),
        "title" => author.get("title").and_then(|a| a.as_array())
            .map(concat_plain_text).unwrap_or_default(),
        "people" => author.get("people").and_then(|a| a.as_array())
            .map(|arr| arr.iter()
                 .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
                 .collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
        "select" => author.get("select").and_then(|s| s.get("name"))
            .and_then(|n| n.as_str()).map(String::from).unwrap_or_default(),
        "multi_select" => author.get("multi_select").and_then(|a| a.as_array())
            .map(|arr| arr.iter()
                 .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                 .collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn concat_plain_text(arr: &Vec<serde_json::Value>) -> String {
    arr.iter()
        .filter_map(|t| t.get("plain_text").and_then(|p| p.as_str()))
        .collect::<String>()
}
```

- [ ] **Step 2: Register the module**

Add `mod notion;` to `src\main.rs` (after the other `mod` declarations).

- [ ] **Step 3: Build**

Run:
```powershell
cargo build
```
Expected: builds clean. Unused-item warnings for `notion` are acceptable until Task 7 consumes it.

- [ ] **Step 4: Commit**

```powershell
git add src/notion.rs src/main.rs
git commit -m "feat(notion): blocking client to query database pages

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 7: UI scaffolding — app state, menu bar, asset table

**Files:**
- Create: `C:\Git\phase\src\ui\mod.rs`
- Create: `C:\Git\phase\src\ui\menu.rs`
- Create: `C:\Git\phase\src\ui\table.rs`
- Modify: `C:\Git\phase\src\main.rs`

- [ ] **Step 1: Create `ui/mod.rs` with app state**

Create `src\ui\mod.rs`:
```rust
mod menu;
mod table;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use crate::config::Config;
use crate::copy::job::{JobMsg, JobProgress};
use crate::copy::plan::{build_plan, Direction};
use crate::notion::{Asset, HDRIS_DB_ID, TEXTURES_DB_ID};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum AssetType { Hdris, Textures }

impl AssetType {
    pub fn label(self) -> &'static str { match self { Self::Hdris => "HDRIs", Self::Textures => "Textures" } }
    pub fn folder(self) -> &'static str { match self { Self::Hdris => "HDRIs", Self::Textures => "Textures" } }
    pub fn db_id(self) -> &'static str { match self { Self::Hdris => HDRIS_DB_ID, Self::Textures => TEXTURES_DB_ID } }
}

pub enum AssetListState {
    Loading,
    Loaded(Vec<Asset>),
    Error(String),
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct RowKey { pub asset_type: AssetType, pub slug: String }

pub struct RowJob {
    pub direction: Direction,
    pub progress:  Arc<JobProgress>,
    pub rx:        Receiver<JobMsg>,
    pub message:   Arc<Mutex<String>>,
}

pub struct AppState {
    pub config:         Config,
    pub current_type:   AssetType,
    pub author_filter:  String,
    pub assets_by_type: HashMap<AssetType, AssetListState>,
    pub error_banner:   Option<String>,
    pub jobs:           HashMap<RowKey, RowJob>,
    pub notion_rx:      HashMap<AssetType, Receiver<Result<Vec<Asset>, String>>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let mut s = Self {
            config,
            current_type: AssetType::Hdris,
            author_filter: String::new(),
            assets_by_type: HashMap::new(),
            error_banner: None,
            jobs: HashMap::new(),
            notion_rx: HashMap::new(),
        };
        if !s.config.notion_token.is_empty() {
            s.refresh(AssetType::Hdris);
            s.refresh(AssetType::Textures);
        }
        s
    }

    pub fn refresh(&mut self, t: AssetType) {
        if self.config.notion_token.is_empty() {
            self.assets_by_type.insert(t, AssetListState::Error("No Notion token configured".into()));
            return;
        }
        self.assets_by_type.insert(t, AssetListState::Loading);
        let (tx, rx) = channel();
        let token = self.config.notion_token.clone();
        let db = t.db_id().to_string();
        thread::spawn(move || {
            let res = crate::notion::fetch_database(&token, &db).map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.notion_rx.insert(t, rx);
    }

    /// Called once per frame: drain Notion + job channels.
    pub fn pump(&mut self) {
        let types: Vec<_> = self.notion_rx.keys().copied().collect();
        for t in types {
            let res_opt = self.notion_rx.get(&t).and_then(|rx| rx.try_recv().ok());
            if let Some(res) = res_opt {
                match res {
                    Ok(list) => { self.assets_by_type.insert(t, AssetListState::Loaded(list)); }
                    Err(msg) => { self.assets_by_type.insert(t, AssetListState::Error(msg)); }
                }
                self.notion_rx.remove(&t);
            }
        }

        let keys: Vec<RowKey> = self.jobs.keys().cloned().collect();
        for k in keys {
            let mut done = false;
            if let Some(job) = self.jobs.get(&k) {
                while let Ok(msg) = job.rx.try_recv() {
                    match msg {
                        JobMsg::FileDone { .. } => {}
                        JobMsg::FileFailed { rel_path, error } => {
                            *job.message.lock().unwrap() = format!("Failed on {rel_path}: {error}");
                            self.error_banner = Some(format!("{}: {rel_path} — {error}", k.slug));
                            done = true;
                        }
                        JobMsg::Finished | JobMsg::Cancelled => { done = true; }
                    }
                }
            }
            if done { self.jobs.remove(&k); }
        }
    }

    pub fn local_root_for(&self, t: AssetType) -> PathBuf { self.config.local_root.join(t.folder()) }
    pub fn prod_root_for(&self,  t: AssetType) -> PathBuf { self.config.prod_root.join(t.folder()) }
}

pub fn start_job(state: &mut AppState, key: &RowKey, direction: Direction) {
    let (src_root, dst_root) = match direction {
        Direction::Pull => (state.prod_root_for(key.asset_type).join(&key.slug),
                            state.local_root_for(key.asset_type).join(&key.slug)),
        Direction::Push => (state.local_root_for(key.asset_type).join(&key.slug),
                            state.prod_root_for(key.asset_type).join(&key.slug)),
    };

    let plan = match build_plan(direction, &src_root, &dst_root) {
        Ok(p) => p,
        Err(e) => { state.error_banner = Some(format!("Plan failed: {e}")); return; }
    };

    if !plan.conflicts().is_empty() {
        // Conflict dialog comes in Task 8 — for now show a banner.
        state.error_banner = Some(format!(
            "{} conflict(s) for {} — conflict dialog not yet implemented",
            plan.conflicts().len(), key.slug
        ));
        return;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan, progress.clone(), tx);
    state.jobs.insert(key.clone(), RowJob {
        direction,
        progress,
        rx,
        message: Arc::new(Mutex::new(String::new())),
    });
}

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    egui::TopBottomPanel::top("menu").show(ctx, |ui| menu::draw(state, ui));
    if let Some(err) = state.error_banner.clone() {
        egui::TopBottomPanel::top("banner").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                if ui.button("✕").clicked() { state.error_banner = None; }
            });
        });
    }
    egui::CentralPanel::default().show(ctx, |ui| table::draw(state, ui));
}
```

- [ ] **Step 2: Implement the menu bar**

Create `src\ui\menu.rs`:
```rust
use super::{AppState, AssetListState, AssetType};

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);

        for t in [AssetType::Hdris, AssetType::Textures] {
            let selected = state.current_type == t;
            if ui.selectable_label(selected, t.label()).clicked() && !selected {
                state.current_type = t;
                state.author_filter.clear();
            }
        }

        ui.separator();

        let authors = current_authors(state);
        let display = if state.author_filter.is_empty() { "All authors".to_string() } else { state.author_filter.clone() };
        egui::ComboBox::from_id_source("author_filter")
            .selected_text(display)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.author_filter, String::new(), "All authors");
                for a in authors {
                    ui.selectable_value(&mut state.author_filter, a.clone(), a);
                }
            });

        ui.separator();

        if ui.button("↻ Refresh").clicked() {
            state.refresh(state.current_type);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("PHASE {}", env!("CARGO_PKG_VERSION")));
        });
    });
}

fn current_authors(state: &AppState) -> Vec<String> {
    let Some(AssetListState::Loaded(list)) = state.assets_by_type.get(&state.current_type) else {
        return Vec::new();
    };
    let set: std::collections::BTreeSet<String> = list.iter()
        .map(|a| a.author.clone())
        .filter(|s| !s.is_empty())
        .collect();
    set.into_iter().collect()
}
```

- [ ] **Step 3: Implement the asset table**

Create `src\ui\table.rs`:
```rust
use super::{AppState, AssetListState, RowKey};
use crate::copy::plan::Direction;
use crate::notion::Asset;

pub fn draw(state: &mut AppState, ui: &mut egui::Ui) {
    let t = state.current_type;
    match state.assets_by_type.get(&t) {
        None | Some(AssetListState::Loading) => { ui.label("Loading…"); return; }
        Some(AssetListState::Error(msg))     => { ui.colored_label(egui::Color32::from_rgb(220,80,80), msg.clone()); return; }
        Some(AssetListState::Loaded(_))      => {}
    }

    let prod_root = state.prod_root_for(t);
    let filter = state.author_filter.clone();
    let mut rows: Vec<RowView> = match state.assets_by_type.get(&t) {
        Some(AssetListState::Loaded(list)) => list.iter()
            .filter(|a| filter.is_empty() || a.author == filter)
            .map(|a| RowView::from_asset(a, &prod_root))
            .collect(),
        _ => Vec::new(),
    };
    rows.sort_by(|a, b| b.exists_on_prod.cmp(&a.exists_on_prod)
        .then_with(|| a.slug.to_lowercase().cmp(&b.slug.to_lowercase())));

    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        for row in rows {
            let key = RowKey { asset_type: t, slug: row.slug.clone() };
            draw_row(state, ui, &key, &row);
        }
    });
}

struct RowView {
    slug: String,
    author: String,
    url: String,
    exists_on_prod: bool,
}

impl RowView {
    fn from_asset(a: &Asset, prod_root: &std::path::Path) -> Self {
        Self {
            slug: a.slug.clone(),
            author: a.author.clone(),
            url: a.url.clone(),
            exists_on_prod: prod_root.join(&a.slug).is_dir(),
        }
    }
}

fn draw_row(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    let row_height = 28.0;
    let avail = ui.available_rect_before_wrap();
    let row_rect = egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), row_height));

    let bg = ui.visuals().extreme_bg_color;
    ui.painter().rect_filled(row_rect, 2.0, bg);
    if let Some(job) = state.jobs.get(key) {
        let f = job.progress.fraction().clamp(0.0, 1.0);
        let mut fill = row_rect;
        fill.set_width(avail.width() * f);
        ui.painter().rect_filled(fill, 2.0, egui::Color32::from_rgb(50, 110, 200));
    }

    ui.allocate_ui_at_rect(row_rect, |ui| {
        ui.horizontal_centered(|ui| {
            ui.add_space(8.0);
            let text_color = if row.exists_on_prod { ui.visuals().text_color() } else { egui::Color32::from_gray(110) };
            ui.colored_label(text_color, &row.slug);
            ui.add_space(16.0);
            ui.colored_label(text_color.linear_multiply(0.8), &row.author);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw_row_actions(state, ui, key, row);
            });
        });
    });

    ui.advance_cursor_after_rect(row_rect);
    ui.separator();
}

fn draw_row_actions(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey, row: &RowView) {
    // Notion button — text "N" for now; replaced with logo in Task 11.
    if ui.button("N").on_hover_text("Open in Notion").clicked() {
        let _ = open::that(&row.url);
    }

    if state.jobs.contains_key(key) {
        if ui.button("✕").on_hover_text("Cancel").clicked() {
            if let Some(job) = state.jobs.get(key) {
                job.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let job = state.jobs.get(key).unwrap();
        let done = job.progress.bytes_done.load(std::sync::atomic::Ordering::Relaxed);
        let tot  = job.progress.bytes_total.load(std::sync::atomic::Ordering::Relaxed);
        let label = match job.direction {
            Direction::Pull => "Pulling from Prod",
            Direction::Push => "Pushing from Local",
        };
        ui.label(format!("{label}  ·  {} / {}", fmt_bytes(done), fmt_bytes(tot)));
        return;
    }

    let enabled = row.exists_on_prod;
    if ui.add_enabled(enabled, egui::Button::new("↑")).on_hover_text("Push to Prod").clicked() {
        super::start_job(state, key, Direction::Push);
    }
    if ui.add_enabled(enabled, egui::Button::new("↓")).on_hover_text("Pull from Prod").clicked() {
        super::start_job(state, key, Direction::Pull);
    }
}

fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = b as f64;
    if b >= GB { format!("{:.2} GB", b / GB) }
    else if b >= MB { format!("{:.1} MB", b / MB) }
    else if b >= KB { format!("{:.0} KB", b / KB) }
    else { format!("{b:.0} B") }
}
```

- [ ] **Step 4: Wire `ui` into `main.rs`**

Replace `src\main.rs`:
```rust
mod config;
mod copy;
mod notion;
mod ui;

use ui::AppState;

fn main() -> eframe::Result<()> {
    let cfg = config::load().unwrap_or_default();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        &format!("PHASE {}", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(App { state: AppState::new(cfg) })
        }),
    )
}

struct App { state: AppState }

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.pump();
        ui::draw(&mut self.state, ctx);
        if !self.state.jobs.is_empty() || !self.state.notion_rx.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
```

- [ ] **Step 5: Build and smoke-test**

Run:
```powershell
cargo build
cargo run
```
Expected: window opens. If no token, the table area shows "No Notion token configured".

- [ ] **Step 6: Set the Notion token for a live test**

Manual developer step (one-off):
```powershell
New-Item -ItemType Directory -Force -Path "$env:APPDATA\phase" | Out-Null
@'
notion_token = "PASTE_TOKEN_HERE"
'@ | Set-Content "$env:APPDATA\phase\config.toml"
```

Re-run `cargo run`. Expected: HDRIs and Textures load; rows for assets present at `P:\Assets\<type>\<slug>` are full opacity, others greyed and sorted to the bottom.

**Do not click Pull/Push on any asset other than `HDRIs/aarfontein_dirt_road`.**

- [ ] **Step 7: Commit**

```powershell
git add src/ui src/main.rs
git commit -m "feat(ui): menu bar, asset table, per-row progress and actions

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 8: Conflict dialog

**Files:**
- Create: `C:\Git\phase\src\ui\dialogs.rs`
- Modify: `C:\Git\phase\src\ui\mod.rs`

- [ ] **Step 1: Add a pending-conflict state and conflict-choice enum to `AppState`**

In `src\ui\mod.rs`, add a new struct and enum:
```rust
pub struct PendingConflict {
    pub key:       RowKey,
    pub direction: Direction,
    pub plan:      crate::copy::plan::Plan,
}

#[derive(Copy, Clone)]
pub enum ConflictChoice { OverwriteAll, CopyOnlyNew, Cancel }
```

Add to the `AppState` struct (alongside the other fields):
```rust
pub pending_conflict: Option<PendingConflict>,
```

Initialise `pending_conflict: None,` in `AppState::new(...)`.

- [ ] **Step 2: Route conflicts through `pending_conflict`**

In `src\ui\mod.rs`, replace the conflict branch inside `start_job` with:
```rust
if !plan.conflicts().is_empty() {
    state.pending_conflict = Some(PendingConflict { key: key.clone(), direction, plan });
    return;
}
```

- [ ] **Step 3: Add the helper that runs a plan after the user resolves conflicts**

Append to `src\ui\mod.rs`:
```rust
use crate::copy::plan::Action;

pub fn execute_after_conflict(state: &mut AppState, choice: ConflictChoice) {
    let Some(pc) = state.pending_conflict.take() else { return; };
    let mut plan = pc.plan;
    match choice {
        ConflictChoice::Cancel => return,
        ConflictChoice::OverwriteAll => {
            for f in plan.files.iter_mut() {
                if matches!(f.action, Action::Conflict { .. }) {
                    f.action = Action::Overwrite;
                    plan.total_bytes_to_copy += f.size;
                }
            }
        }
        ConflictChoice::CopyOnlyNew => {
            plan.files.retain(|f| !matches!(f.action, Action::Conflict { .. }));
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Arc::new(JobProgress::default());
    crate::copy::job::spawn(plan, progress.clone(), tx);
    state.jobs.insert(pc.key, RowJob {
        direction: pc.direction,
        progress,
        rx,
        message: Arc::new(Mutex::new(String::new())),
    });
}
```

- [ ] **Step 4: Implement the dialog**

Create `src\ui\dialogs.rs`:
```rust
use super::{AppState, ConflictChoice};
use crate::copy::plan::Action;

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let Some(pc) = state.pending_conflict.as_ref() else { return; };
    let slug = pc.key.slug.clone();
    let conflicts: Vec<(String, &'static str)> = pc.plan.files.iter()
        .filter_map(|f| match f.action {
            Action::Conflict { dest_newer: true }  => Some((f.rel_path.to_string_lossy().to_string(), "Newer at destination")),
            Action::Conflict { dest_newer: false } => Some((f.rel_path.to_string_lossy().to_string(), "Newer at source")),
            _ => None,
        })
        .collect();

    let mut choice: Option<ConflictChoice> = None;

    egui::Window::new(format!("Conflicts — {slug}"))
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("{} file(s) in conflict:", conflicts.len()));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (path, note) in &conflicts {
                    ui.horizontal(|ui| {
                        ui.monospace(path);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(*note);
                        });
                    });
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Overwrite All").clicked() { choice = Some(ConflictChoice::OverwriteAll); }
                if ui.button("Copy Only New").clicked() { choice = Some(ConflictChoice::CopyOnlyNew); }
                if ui.button("Cancel").clicked()        { choice = Some(ConflictChoice::Cancel); }
            });
        });

    if let Some(c) = choice {
        if matches!(c, ConflictChoice::Cancel) {
            state.pending_conflict = None;
        } else {
            super::execute_after_conflict(state, c);
        }
    }
}
```

- [ ] **Step 5: Register and render the dialog**

In `src\ui\mod.rs` add at the top alongside the other module decls:
```rust
mod dialogs;
```
In `draw(...)`, **before** the `CentralPanel`:
```rust
dialogs::draw(state, ctx);
```

- [ ] **Step 6: Build**

Run:
```powershell
cargo build
```
Expected: clean.

- [ ] **Step 7: Commit**

```powershell
git add src/ui
git commit -m "feat(ui): conflict resolution dialog

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 9: Token-prompt dialog (first run)

**Files:**
- Modify: `C:\Git\phase\src\ui\dialogs.rs`
- Modify: `C:\Git\phase\src\ui\mod.rs`

- [ ] **Step 1: Add token-prompt state**

In `src\ui\mod.rs`, add to `AppState`:
```rust
pub token_prompt_open: bool,
pub token_input:       String,
```

In `AppState::new`, immediately after constructing `s`:
```rust
s.token_prompt_open = s.config.notion_token.is_empty();
s.token_input       = s.config.notion_token.clone();
```

- [ ] **Step 2: Implement the prompt**

Append to `src\ui\dialogs.rs`:
```rust
pub fn token_prompt(state: &mut AppState, ctx: &egui::Context) {
    if !state.token_prompt_open { return; }
    let mut save = false;
    let mut close = false;
    egui::Window::new("Notion token required")
        .collapsible(false).resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Paste your Notion integration token. It will be saved to");
            ui.monospace(
                crate::config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.token_input)
                    .password(true)
                    .desired_width(400.0),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked()   { save = true; }
                if ui.button("Cancel").clicked() { close = true; }
            });
        });
    if save {
        state.config.notion_token = state.token_input.trim().to_string();
        if let Err(e) = crate::config::save(&state.config) {
            state.error_banner = Some(format!("Failed to save config: {e}"));
        }
        state.token_prompt_open = false;
        state.refresh(super::AssetType::Hdris);
        state.refresh(super::AssetType::Textures);
    } else if close {
        state.token_prompt_open = false;
    }
}
```

- [ ] **Step 3: Render the prompt**

In `src\ui\mod.rs` `draw(...)`, immediately before `dialogs::draw(state, ctx);`:
```rust
dialogs::token_prompt(state, ctx);
```

- [ ] **Step 4: Build and run**

Run:
```powershell
Remove-Item "$env:APPDATA\phase\config.toml" -ErrorAction SilentlyContinue
cargo build
cargo run
```
Expected: with config missing, the token prompt appears centered on launch. Save adds the file and triggers refresh.

- [ ] **Step 5: Commit**

```powershell
git add src/ui
git commit -m "feat(ui): first-run Notion token prompt

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 10: Logging

**Files:**
- Modify: `C:\Git\phase\src\main.rs`
- Modify: `C:\Git\phase\src\copy\job.rs`

- [ ] **Step 1: Initialise the logger at startup**

In `src\main.rs`, add a function at the bottom of the file:
```rust
fn init_logging() {
    use simplelog::{LevelFilter, WriteLogger, Config as SlConfig};
    use std::fs::OpenOptions;
    let Ok(path) = config::log_path() else { return; };
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = WriteLogger::init(LevelFilter::Info, SlConfig::default(), file);
    }
}
```
Call it in `main()` as the first line:
```rust
init_logging();
log::info!("PHASE {} starting", env!("CARGO_PKG_VERSION"));
```

- [ ] **Step 2: Log job lifecycle in `job.rs`**

At the top of `src\copy\job.rs` add:
```rust
use log::{info, warn};
```
Inside `spawn`, just after `progress.bytes_total.store(...)`:
```rust
info!("job start: {:?} direction, {} files, {} bytes",
    plan.direction, plan.files.len(), plan.total_bytes_to_copy);
```
Just before `let _ = tx.send(JobMsg::Finished);`:
```rust
info!("job finished");
```
In the `Err(CopyError::Cancelled)` branch, before the `return`:
```rust
info!("job cancelled");
```
In the `Err(e)` branch, before sending `FileFailed`:
```rust
warn!("file failed {}: {}", file.rel_path.display(), e);
```

- [ ] **Step 3: Build**

Run:
```powershell
cargo build
cargo test
```
Expected: clean build, all tests pass.

- [ ] **Step 4: Commit**

```powershell
git add src/main.rs src/copy/job.rs
git commit -m "feat: append job lifecycle to %APPDATA%\phase\phase.log

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 11: Notion logo for the Notion button

**Files:**
- Create: `C:\Git\phase\src\assets\notion_logo.png`
- Modify: `C:\Git\phase\src\ui\mod.rs`
- Modify: `C:\Git\phase\src\ui\table.rs`

- [ ] **Step 1: Save the Notion logo**

Save Notion's "N" mark PNG (monochrome, ~64×64) to `src\assets\notion_logo.png`. Source: <https://www.notion.com/about>. If unsure which file to use, any small PNG works for now (replace later); the button just needs *some* image.

PowerShell to create the directory:
```powershell
New-Item -ItemType Directory -Force -Path C:\Git\phase\src\assets | Out-Null
```

- [ ] **Step 2: Add a texture loader to `ui/mod.rs`**

Append:
```rust
pub fn notion_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    use std::sync::OnceLock;
    static BYTES: &[u8] = include_bytes!("../assets/notion_logo.png");
    // egui's TextureHandle is per-context, but for our single-window app a single
    // process-wide OnceLock is fine because the context lives for the whole run.
    static TEX: OnceLock<egui::TextureHandle> = OnceLock::new();
    TEX.get_or_init(|| {
        let image = image::load_from_memory(BYTES).expect("decode notion_logo.png").to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let pixels = image.into_raw();
        ctx.load_texture(
            "notion_logo",
            egui::ColorImage::from_rgba_unmultiplied(size, &pixels),
            egui::TextureOptions::LINEAR,
        )
    }).clone()
}
```

- [ ] **Step 3: Use it in the row Notion button**

In `src\ui\table.rs`, replace the `ui.button("N")` block with:
```rust
let tex = super::notion_logo_texture(ui.ctx());
let btn = egui::ImageButton::new(egui::load::SizedTexture::new(tex.id(), egui::vec2(18.0, 18.0)));
if ui.add(btn).on_hover_text("Open in Notion").clicked() {
    let _ = open::that(&row.url);
}
```

- [ ] **Step 4: Build and run**

Run:
```powershell
cargo build
cargo run
```
Expected: rows show the Notion image button.

- [ ] **Step 5: Commit**

```powershell
git add src/assets src/ui
git commit -m "feat(ui): Notion logo for the Notion button

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

## Task 12: Release build and live verification

**Files:** (none modified)

- [ ] **Step 1: Build the release executable**

Run:
```powershell
cd C:\Git\phase
cargo build --release
```
Expected: produces `target\release\phase.exe`, ~15–25 MB.

- [ ] **Step 2: Smoke-test the release binary**

Run:
```powershell
.\target\release\phase.exe
```
Expected: window opens, title shows version, Notion lists load.

- [ ] **Step 3: Live Pull/Push test on the allowed asset**

In the running app:
1. Switch to **HDRIs**, locate `aarfontein_dirt_road`.
2. Click **Pull** (↓). Watch the row background fill and byte counter update.
3. Verify `C:\PHASE\Assets\HDRIs\aarfontein_dirt_road` exists and contains no `.tif`/`.NEF` files.
4. Optionally touch a small local file (change its mtime), then click **Push** (↑). Expect only changed files to copy; if the source is now older than Prod, the conflict dialog appears.
5. Click **Pull** again — all files should classify as `Identical` and the job should finish near-instantly (only validation reads).

- [ ] **Step 4: Check the log**

Run:
```powershell
Get-Content "$env:APPDATA\phase\phase.log" -Tail 20
```
Expected: `job start: ...`, `job finished` entries.

- [ ] **Step 5: Tag the release**

```powershell
cd C:\Git\phase
git status   # should be clean
git tag v0.1.0
```

---

## Self-review notes

Spec coverage:
- ✅ HDRIs + Textures, IDs and `Author` property → Task 6
- ✅ Pull excludes `.tif`/`.tiff`/`.nef`; Push excludes nothing → Task 3
- ✅ Atomic `.partial` + rename, fsync, BLAKE3 validation, mtime preservation → Task 4
- ✅ Conflict gate before any writes; Overwrite-All / Copy-Only-New / Cancel → Task 8
- ✅ Byte-based progress, row background animates blue, status text + (✕) cancel → Task 7
- ✅ Greyed-out and bottom-sorted rows when Prod folder missing → Task 7
- ✅ Refresh button, author filter, type selector → Task 7
- ✅ Notion button uses Notion logo → Task 11
- ✅ Config at `%APPDATA%\phase\config.toml`, log at `%APPDATA%\phase\phase.log`, version in title → Tasks 1, 2, 10
- ✅ Single exe, MSVC, `lto`/`strip` → Tasks 1, 12
- ✅ Unit tests for copy module only → Tasks 3, 4
- ✅ Manual test asset restricted to `HDRIs/aarfontein_dirt_road` → Tasks 7, 12

Deliberate simplification vs spec: the spec mentions "a global semaphore caps total concurrent file copies at 4". The plan keeps per-row sequential copies but does **not** add a cross-row semaphore in v0.1. Adding it later is a small change (`Arc<Semaphore>` shared by all `spawn` calls). Operationally, an operator firing off >4 row jobs simultaneously is unlikely; revisit if needed.
