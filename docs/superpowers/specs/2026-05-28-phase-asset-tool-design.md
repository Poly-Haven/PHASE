# PHASE — Poly Haven Asset System, etc. (v0.1) — Design

**Date:** 2026-05-28
**Status:** Approved (brainstorming complete, ready for implementation plan)

## Purpose

An internal Windows desktop utility for a small production team to manage
asset files between the production NAS (`P:\Assets`) and local working
folders (`C:\PHASE\Assets`). The first version handles two asset types
(HDRIs and Textures) and exposes per-asset **Pull**, **Push**, and
**Notion** actions.

This is intended to grow into a broader production pipeline tool (SD-card
ingest, validation, review, upload prep), but v0.1 focuses solely on
robust Pull/Push backed by Notion-sourced asset listings.

## Sources of truth

- **Production NAS (`P:\Assets\<type>\<slug>`)** — authoritative for
  finalized asset *file data*.
- **Notion databases** — authoritative for asset *metadata* and the list
  of assets that exist. Notion does not describe filesystem state.
- **Local working folders (`C:\PHASE\Assets\<type>\<slug>`)** — temporary
  working copies.

## Tech stack

- **Language:** Rust (stable, MSVC toolchain)
- **UI:** `eframe` / `egui` (immediate-mode, dark theme, single binary)
- **HTTP:** `reqwest` blocking + `rustls`
- **JSON:** `serde` / `serde_json`
- **Config:** `toml`
- **Filesystem:** `walkdir`, `filetime`, `std::fs`
- **Hashing:** `blake3`
- **Misc:** `open` (launch URLs), `dirs` (locate `%APPDATA%`), `anyhow`,
  `log` + `simplelog`

No async runtime. Background work runs on OS threads, communicates via
`std::sync::mpsc` channels and `Arc<AtomicU64>` counters.

## Architecture

Single-process Windows GUI executable (`phase.exe`). Three internal
modules with clear boundaries:

- **`notion`** — fetches and caches asset lists from the two databases.
- **`copy`** — robust file-tree copy with atomic temp+rename semantics,
  conflict detection, hash validation, and progress reporting.
- **`ui`** — egui frontend. Spawns background work, samples shared
  progress state each frame.

### Project layout

```
phase/
├── Cargo.toml
├── src/
│   ├── main.rs                 # entry, eframe setup, dark theme
│   ├── config.rs               # load/save %APPDATA%\phase\config.toml
│   ├── notion.rs               # API client, query, parse
│   ├── copy/
│   │   ├── mod.rs              # public API: plan(), execute()
│   │   ├── plan.rs             # walk source, classify, detect conflicts
│   │   ├── engine.rs           # streaming copy + blake3 validation
│   │   └── tests.rs            # unit tests using tempdir
│   ├── ui/
│   │   ├── mod.rs              # main app state
│   │   ├── menu.rs             # top bar
│   │   ├── table.rs            # asset rows, progress rendering
│   │   └── dialogs.rs          # conflict dialog, token dialog
│   └── assets/
│       └── notion_logo.png     # embedded via include_bytes!
└── docs/superpowers/specs/
```

## Configuration

Stored at `%APPDATA%\phase\config.toml`. Schema:

```toml
notion_token = "secret_xxx"
# Optional overrides; defaults shown:
# prod_root  = "P:\\Assets"
# local_root = "C:\\PHASE\\Assets"
```

On first run, if the file is missing or the token is empty, the app
shows a small modal asking for the Notion integration token, writes the
file, and continues.

Log file: `%APPDATA%\phase\phase.log` (append-only, plain text, rolling
size cap).

## Notion integration

Two databases:

| Type     | Database ID                              |
|----------|------------------------------------------|
| HDRIs    | `21f373ac-61c1-80d0-8e55-cd46d121d1d5`   |
| Textures | `215373ac-61c1-80dd-8a97-edb25bb6a5f8`   |

Per page we read:

- **Title** → used as the **slug** (folder name on disk). No display
  name is ever used; the slug is always shown verbatim.
- **`Author`** property (named identically in both DBs).
- **`url`** field on the page object → used by the **Notion** button.

API: `POST https://api.notion.com/v1/databases/{id}/query`, paginated
until `has_more` is `false`. Standard `Authorization: Bearer <token>`
and `Notion-Version` headers.

The fetch runs on a background thread on launch, on type switch, and
on user request via the **Refresh** (↻) button next to the type
selector. Results are cached in memory for the session.

Errors (bad token, network down, schema mismatch) appear as an inline
banner under the menu bar with the message. No modal popups.

## UI

Single window, dark theme.

### Menu bar (top row)
- Segmented asset-type selector: `[HDRIs | Textures]`
- Author filter dropdown (populated from the current type's assets,
  default `All`)
- Refresh button (↻)

### Main area — asset table

Columns: `Slug | Author | Actions`

- Slug: left-aligned, plain text.
- Author: secondary text color.
- Actions: three icon buttons in order:
  - **Pull** (↓)
  - **Push** (↑)
  - **Notion** (uses Notion logo from embedded PNG; opens page in
    default browser via the `open` crate)

Rows where `P:\Assets\<type>\<slug>` does not exist are **greyed
out**, Pull/Push are disabled, and they sort to the bottom of the
list.

### Row progress rendering

While a Pull or Push runs on a row:

- The row's background becomes the progress bar itself — dark grey
  background with a blue fill animating from left to right in
  proportion to bytes copied / total bytes.
- The action buttons are replaced by plain text status:
  `Pulling from Prod  ·  1.3 / 4.6 GB` (or `Pushing from Local`).
- An `(x)` cancel button is shown at the right of the row.

Progress is byte-based, sampled from a shared `AtomicU64` each frame.

## Copy engine

Two phases: **plan** (read-only, fast), then **execute** (after any
conflict gate).

### Phase 1 — Plan (mtime + size only, no hashing)

1. Walk the source tree recursively (`walkdir`).
2. **Pull only**: skip files whose extension matches (case-insensitive)
   `.tif`, `.tiff`, or `.nef`. Push excludes nothing.
3. For each source file, stat the destination and classify:
   - **new** (no dest) → copy
   - **source mtime > dest mtime**, or different size → copy
     (overwrite)
   - **dest mtime > source mtime** → conflict
   - **identical** (same size, mtime within ±2s) → skip
4. Sum total bytes to copy.

mtime tolerance accounts for filesystem precision differences (NTFS vs
SMB).

### Phase 2 — Conflict gate

If any conflicts exist, show a single modal **before any writes**:

- Scrollable list of conflicting files with plain-English notes
  (e.g. `Newer on Prod`, `Newer locally`).
- Three buttons: **Overwrite All** / **Copy Only New** / **Cancel**.

`Copy Only New` drops the conflicting files from the plan; everything
else proceeds.

### Phase 3 — Execute (background worker thread per row)

For each file in the plan:

1. Create parent directories as needed.
2. Open the source for reading and `<dest>.partial` for writing
   (truncating).
3. Initialise a BLAKE3 hasher.
4. Loop in 8 MiB chunks:
   - Read from source.
   - Feed the chunk into the hasher.
   - Write the chunk to `.partial`.
   - Add chunk size to the shared `AtomicU64` byte counter.
   - Check the cancel flag; if set, close + delete `.partial` and
     abort the row.
5. `flush` and `sync_all` (fsync) the `.partial` file.
6. Finalise the source-side hash → `H_src`.
7. **Validation pass**: reopen `.partial`, stream-hash in 8 MiB
   chunks → `H_dst`.
8. If `H_src == H_dst`:
   - `fs::rename(<dest>.partial, <dest>)` — atomic on NTFS for
     same-volume renames.
   - Set destination mtime to match source mtime (`filetime`).
9. Else: delete `.partial`, log a corruption error, mark the row as
   failed.

### Concurrency

- Multiple rows may run Pull/Push in parallel (independent operations).
- A **global semaphore caps total concurrent file copies at 4** to
  avoid saturating the NAS / SMB stack.
- Within a single row, files copy sequentially. This keeps cancel
  semantics simple and disk I/O predictable.

### Crash and interrupt safety

- Only `.partial` files are ever in flight; originals at the
  destination are untouched until the atomic rename.
- If the process dies mid-copy, originals are intact; `.partial`
  files are orphaned and harmless.
- On the next run, the planner ignores `.partial` files. A small
  sweep deletes any `.partial` files it encounters in folders it is
  about to touch.

### Logging

Every operation appends to `%APPDATA%\phase\phase.log`: start
timestamp, type, slug, direction, file count, total bytes, conflict
counts, completion timestamp, and any errors.

## Build & distribution

- `cargo build --release` produces `target/release/phase.exe`.
- Target triple: `x86_64-pc-windows-msvc`. Built natively on Windows.
- `Cargo.toml` release profile: `lto = true`, `strip = true`,
  `codegen-units = 1`.
- Distribution: ship the single `phase.exe` over the NAS or chat. No
  installer, no auto-update, no code signing for v0.1.
- Version string from `env!("CARGO_PKG_VERSION")` shown in the window
  title (e.g. `PHASE 0.1.0`).
- All persistent state lives in `%APPDATA%\phase\`. Nothing is written
  next to the executable. This keeps the door open for a future
  auto-updater.

## Testing

- **Unit tests for the `copy` module only** — this is the highest-risk
  area (data corruption is the worst failure mode). Tests cover:
  classification rules, exclusion rules (Pull skips `.tif`/`.NEF`),
  conflict detection, atomic rename, validation failure handling
  (simulate by tampering with `.partial` mid-flight), and cancel.
  Uses `tempfile` for isolated working directories.
- **No automated tests for `notion` or `ui`.** Manual verification for
  these layers.
- **Manual test asset for live filesystem testing:**
  `HDRIs/aarfontein_dirt_road` only.

## Out of scope for v0.1

- Models database (deferred).
- Auto-update mechanism.
- Code signing.
- SD-card ingest, validation reports, review tooling, upload prep
  (all later phases).
- Bulk-select / multi-row operations.
- Localisation, accessibility audits.
