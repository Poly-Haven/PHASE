# AGENTS.md — PHASE

PHASE is a **Windows desktop app** (Rust + `egui`/`eframe` 0.27) for the Poly Haven team to move
**HDRI/Texture asset folders** between the Local working drive, the production NAS, and an archive
drive, while keeping each asset's workflow status in sync with the Notion-backed admin backend.
It also ingests photos off camera memory cards onto the NAS (see **Card ingest**).

Licensed **GPL-3.0-or-later** (see `LICENSE`); `NOTICE` records the LGPL-2.1 `rawler` dependency.

## Build / run / test
- `cargo build` → `target\debug\phase.exe`; `cargo build --release` → `target\release\phase.exe`.
- `cargo test` (~270 tests, fast). `cargo clippy` has ~9 **pre-existing** warnings (auth/notion/mod) — don't treat as new.
- `[profile.release]` deliberately does **not** set `panic = "abort"`: card ingest wraps every RAW
  decode in `catch_unwind` because `rawler` panics on some malformed files, and detecting those
  without taking PHASE down with them is the entire point of decoding them.
- Toolchain: **Rust MSVC** (Windows-only; `build.rs` sets the icon via `winres`, `main.rs` uses the windows subsystem).
- **Gotcha:** if the app is running it holds a lock on `target\debug\phase.exe`, so `cargo build` fails at link with `Access is denied (os error 5)`. Stop the process first (`Stop-Process -Name phase -Force`); compilation already succeeded if you see this.
- Debug builds hit the backend at `http://localhost:3001/`; release builds hit `https://admin.polyhaven.com/` (`auth::phase_api_base_url`).
- Runtime data in `%APPDATA%\phase\`: `config.toml` (+`.bak`), `phase.log`, `cache\thumbnails\`.

## Architecture
- `src/main.rs` — `eframe::run_native`; logging via `simplelog`; per-frame `update()` = `pump()` then `draw()`.
- `src/ui/` — all UI. `mod.rs` holds **`AppState`** (≈90-field god-object) + `pump`/`draw`. Other modules: `table` (asset grid + rows), `jobs` (transfer dispatch), `scripts` (context menu + admin HDRI scripts), `dialogs` (modals/settings), `thumbnails`, `file_watcher`, `validation` glue, `menu`, `layout`, `colors`, `textures` (SVG icon loaders).
- `src/copy/` — transfer engine: `plan` (walk+classify), `job` (worker threads), `engine` (BLAKE3 stream copy + verify).
- `src/validation/` — background asset checks (`root_entries`, `local_freshness`, `needs_review`).
- `src/ingest/` — memory-card ingest: `scan` (walk + classify + pair), `exif` (Make/Model/orientation),
  `thumb` (RAW decode → 64px JPEG), `job` (two-stage worker pools), `manifest`, `log` (card summary).
  UI lives in `src/ui/ingest.rs`.
- `src/{auth,notion,polyhaven,config,cache,slug,updater,removable_media}.rs` — Auth0 login, admin API,
  public-API published-slug cache, config, JSON cache, slug parse/validate, self-update, card detection.

### Concurrency model (important)
All slow work runs on **background threads** and reports back via **`mpsc` channels stored in `AppState`**.
`pump()` drains every channel each frame (jobs, plan_jobs, verifications, archive_deletes, validation, thumbnails,
notion fetch, auth, update check, …). `draw()` requests repaints while work is in flight (incl. while unfocused, for the watcher). Keep this pattern: spawn thread → push receiver into an `AppState` map → drain in `pump()`.

## Domain model
- `notion::Asset { page_id, slug, author(s), author_profiles, url, status: Option<AssetStatus> }`.
- `AssetStatus { id, name, color, group, sort_order }`; `StatusGroup { ToDo, InProgress, Complete }`. **"Done" is a status *name* in the `Complete` group** — gate "finished" features on `group == Complete`, not the name.
- `AssetType { Hdris, Textures }`; `folder()` → `"HDRIs"`/`"Textures"`.
- Path roots (config): `prod_root`=`P:\Assets`, `local_root`=`C:\PHASE`, `archive_root`=`A:\` (note: archive has **no** `Assets` segment). Helpers: `prod_root_for/local_root_for/archive_root_for(AssetType)`.
  - Prod/Local asset dir: `{root}\{HDRIs|Textures}\{slug}`; Archive: `{archive_root}\{HDRIs|Textures}\{slug}`.
  - Per-asset subfolders: `raw`, `staging`, `work`. Primary file: `staging\{slug}.exr` (→`.hdr` fallback) for HDRIs, `staging\{slug}.blend` for Textures (`table::asset_file_path`). Thumbnail: `staging\renders\primary.png`.
- `RowKey { asset_type, slug }` identifies a row/asset everywhere.

## Transfer pipeline (`TransferKind { Push, Pull, Archive, Unarchive }`)
One unified pipeline; differs only by roots, progress color/direction, and post-copy steps:
`start_job`/`start_archive`/`start_unarchive` → **plan_jobs** (thread builds a `Plan`) → `spawn_copy_job` → **jobs** (copy workers) → terminal step. Per-asset concurrency (keyed by `RowKey`); the row's ✕ sets `progress.cancel`.
- **Verify timing** (`copy/engine.rs`, BLAKE3): Pull/Archive/Unarchive verify **inline** per file (`spawn_immediate_verify` / `copy_one_file`); Push copies deferred then runs a **separate** verify pass (`spawn_verification` → `verifications` map, shown by the small bottom-edge bar). Copy-level `Direction { Push, Pull }` only encodes verify timing — don't confuse with `TransferKind`.
- **Archive** = Prod→Archive, skips `*.tif/.tiff` anywhere under `work/` (`build_archive_plan`/`is_work_tif`); after a fully-verified copy it deletes the whole Prod slug folder via **`delete_prod_after_archive`** (a re-scan safety gate that refuses to delete any file not archived/verified and not a work-tif). Then tracked in `archive_deletes`.
- **Unarchive** = Archive→Prod, non-destructive (archive copy kept).
- Conflicts: dialog for Push/Pull; auto-overwrite (source authoritative) for Archive/Unarchive.
- UI: full-row "wash" = copy progress (Push purple-LTR, Pull blue-RTL, Archive/Unarchive green LTR/RTL); small bottom bar = a separate verify step (today only Push).

## Card ingest (`src/ingest/`, `src/ui/ingest.rs`)

Copies every still image off an inserted card to `{ingest_root}\{card}\` (default `P:\_INGEST`,
config `ingest_root`), **mirroring the card's own folder tree**. The card is usually formatted after
a shoot, so the ingested copy is often the only copy — which is why every stage verifies rather than
trusts, one bad file never aborts a run, nothing is dropped without being counted, and a
`phase_log.txt` is left on the card saying whether it is **safe to format**.

- **Detection** — `removable_media::{list_removable_drives, fingerprint}` polled on a background
  thread every 2s focused / 15s not, guarded against stacking. Polling threads must call
  `suppress_no_media_dialogs()`: an empty reader slot still reports `DRIVE_REMOVABLE`, and querying
  it otherwise raises the shell's "please insert a disk" modal. Identity is `signature`; the folder
  name is `friendly_name` (`120GB_671658`). Reformatting a card changes its identity by design.
- **Scan** — deep walk, extension allowlist (`RAW_EXTENSIONS` / `STILL_EXTENSIONS`; `bmp` is
  deliberately excluded because Magic Lantern installs a folder of them). **JPEGs are dropped when a
  RAW of the same stem sits in the same directory** — per-directory on purpose, since two image
  folders may hold different shots under the same name. Camera names come from *sampled* files: one
  per (directory, name prefix, extension, run of consecutive numbers), not one per file.
- **Two stages, shared pools** (`job.rs`) — stage A copies and hashes the source
  (`copy::engine::copy_one_file_hashed`); stage B reads the destination back **once**, checks the
  hash, and decodes the image from the same buffer. Pools are shared across cards so two cards take
  turns over one link. A file marks purple only when a verify worker actually has it — the hand-off
  queue is `COPY_WORKERS` deep so the grid cannot show more busy files than are really in flight.
  Failures retry, then go red; the run continues. Opposite of `copy/job.rs`, which fails fast.
- **Thumbnails** — `thumb.rs` decodes the **actual sensor data** via `rawler`, not the embedded
  preview: corrupt raw payload leaves the preview perfectly readable, so a preview check would pass a
  ruined photo. Only the decode runs at full size; the CFA is box-averaged straight down to 64px and
  rawler's own white-balance/colour-matrix/sRGB maths then runs on a few thousand pixels. Rendered
  **scene-linear with no exposure normalisation** — these are bracketed HDRI shoots, and a dark frame
  should look dark. `exif::looks_like_raw` gates the decoder: handed 4 KB of zeroes, rawler sniffs
  them into some format and allocates past 6 GB, which `catch_unwind` cannot save you from.
- **Idempotency** — `.phase-ingest.json` in the card's ingest folder records the verified BLAKE3 per
  file. A re-run skips instantly when source *and* destination still match their recorded
  `(size, mtime)` within ±2s; a whole-hour shift (FAT/DST) re-hashes the source instead of re-copying.
- **Where the time goes** — measured, not assumed, and the answer is the hardware. Per
  48.7 MB Nikon NEF in a release build: copy card→NAS 690 ms, read back 536 ms, BLAKE3
  11 ms, RAW decode 382 ms. None of those is the limit. The pipeline is bound by the
  **gigabit NIC**, and because each file is written and then read back for verification it
  costs 97.4 MB of traffic per 48.7 MB file — about 1.3 files/s, so ~27 minutes for a
  2120-file card. Things that were measured and did **not** help: doubling either worker
  pool (within noise), skipping the RAW decode entirely (1.29 vs 1.28 files/s — the decode
  is completely hidden behind I/O), and deepening the hand-off queue. A slow card can bind
  instead: on a USB 2 port the same card read a flat 32 MB/s and that became the ceiling, so
  check the card before suspecting the code. `benchmark_ingest_stages`,
  `benchmark_stage_a_concurrency` and `benchmark_dir_cache` in `ingest::job` measure all of
  this against real hardware.
  - Beware when benchmarking this by hand: reading a file back moments after writing it
    looks like a 1.7x penalty, but only if the reader opens the file *while* it is still
    being written. Reading a completed file is no slower whether it was written seconds or
    hours ago. Likewise, re-reading the same files measures the Windows page cache, not the
    network — always read a set you have not touched.
  - The one lever that would actually move the number is halving the traffic by dropping
    the read-back verification, which trades away the guarantee that what landed on the NAS
    is what was read off the card. That is a deliberate product decision, not a tuning knob.
- **Eject** — offered only when every file is green. Many multi-slot readers do not implement media
  eject and Windows' own "Safely Remove" fails on them too; that is reported as a note, not a fault,
  because the data is already verified.

## Supporting subsystems
- **Validation** runs on a worker pool over visible assets, debounced after watcher events; results keyed by `RowKey` in `validation_results`. Checks: `root_entries` (expected `raw/staging/work`), `local_freshness` (local newer than prod when needs-review), `needs_review` (required staging files present for review statuses).
- **File watcher** is activity-aware: real-time `notify` watching when the window is active, polling (≈30s→idle) when not.
- **Thumbnails**: source priority Local → Prod → Archive (Archive only when status is Complete); cached by `(mtime,size)`, pruned >60 days.
- **Admin** (`AppState::is_admin`, from the JWT role claim): admin-only HDRI context-menu scripts `Normalize`/`Render` (Python under `~/Poly Haven Dropbox/Assets/PH Utils/Scripts/HDRIs`).
- **Updates** are silent and automatic (`updater.rs`): PHASE checks GitHub on startup and hourly, and installs anything newer straight over its own exe (`self_replace`) without asking — the running process is untouched, so the new version takes over on the next start. `staged_update` then drives a status-bar "click to restart" prompt, shown **only for a minor/major** bump; patches just turn up. Restarting is `restart_requested` → `draw` closes the window → `main::on_exit` relaunches, so the layout is still saved. `AUTO_UPDATE_ENABLED` disables all of this in debug builds, which would otherwise get a release binary written over `target\debug\phase.exe`.
- **"What's new"** (`ui/changelog.rs`) shows the release notes for every version between `config.last_run_version` and the running one, on the first launch after an update (patches included). `last_run_version` is only recorded once the notes actually arrive, so an offline start retries rather than swallowing them — and a version GitHub has no release for (a local build) is never recorded at all. Notes are parsed by a small hand-rolled Markdown subset; see the release conventions below for what renders.
- **Auth**: Auth0 PKCE browser login, callback `127.0.0.1:45873`; tokens stored (unencrypted) in `config.toml`, auto-refreshed (`auth::ensure_access_token`, 60s buffer).
- **API**: `GET api/phase/assets?type=`, `PATCH api/phase/assets/{page_id}/status`, `…/title`. HTTP via `reqwest::blocking`.

## Conventions
- Commits: `feat:` / `fix:` / etc. prefix.
- Releases are built and published **locally** (no CI — a workflow run took ~10 min for a <1 min job). Bump `version` in `Cargo.toml` (the tag derives from it), commit, push the branch, then `pwsh scripts/release.ps1 -Publish -NotesFile <file>` — it tests, builds, packages `dist\phase-v*-x86_64-pc-windows-msvc.zip`, creates the lightweight `v*` tag, pushes it, and creates the GitHub Release with the zip + bare `phase.exe` attached. Commit/tag/push only when asked.
  - **Write the notes yourself, and keep them extremely short.** PHASE shows them verbatim in a small "What's new" box, and long text simply won't be read. Aim for **≤5 bullets of ≤15 words**, one per user-visible change, grouped under `### Features` / `### Fixes` (drop a group if it's empty, drop the headings entirely for a single-bullet release). No preamble, no "## What's new" heading (the dialog supplies one), no explaining the implementation. Cover everything since the previous tag, but a change nobody would notice doesn't need a bullet. Only `### headings`, `- bullets`, `**bold**` and `` `code` `` render — everything else shows as plain text. Omitting `-NotesFile` writes a placeholder to fix up with `gh release edit v* --notes-file <file>`.
  - Don't rename the zip: the in-app `updater` (`self_update`) finds its download by matching the `x86_64-pc-windows-msvc` target in the asset name, and extracts `phase.exe` from the archive root.
- **`Cargo.lock` is gitignored** (so is `/target` and `/docs/superpowers/`) — version bumps touch only `Cargo.toml`.
- Match surrounding style; new background work should follow the thread→channel→`pump()` pattern, and new per-asset settings go in `config.rs` + the Settings dialog (`dialogs::settings`).
