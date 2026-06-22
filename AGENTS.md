# AGENTS.md — PHASE

PHASE is a **Windows desktop app** (Rust + `egui`/`eframe` 0.27) for the Poly Haven team to move
**HDRI/Texture asset folders** between the Local working drive, the production NAS, and an archive
drive, while keeping each asset's workflow status in sync with the Notion-backed admin backend.

## Build / run / test
- `cargo build` → `target\debug\phase.exe`; `cargo build --release` → `target\release\phase.exe`.
- `cargo test` (~140 tests, fast). `cargo clippy` has ~9 **pre-existing** warnings (auth/notion/mod) — don't treat as new.
- Toolchain: **Rust MSVC** (Windows-only; `build.rs` sets the icon via `winres`, `main.rs` uses the windows subsystem).
- **Gotcha:** if the app is running it holds a lock on `target\debug\phase.exe`, so `cargo build` fails at link with `Access is denied (os error 5)`. Stop the process first (`Stop-Process -Name phase -Force`); compilation already succeeded if you see this.
- Debug builds hit the backend at `http://localhost:3001/`; release builds hit `https://admin.polyhaven.com/` (`auth::phase_api_base_url`).
- Runtime data in `%APPDATA%\phase\`: `config.toml` (+`.bak`), `phase.log`, `cache\thumbnails\`.

## Architecture
- `src/main.rs` — `eframe::run_native`; logging via `simplelog`; per-frame `update()` = `pump()` then `draw()`.
- `src/ui/` — all UI. `mod.rs` holds **`AppState`** (≈90-field god-object) + `pump`/`draw`. Other modules: `table` (asset grid + rows), `jobs` (transfer dispatch), `scripts` (context menu + admin HDRI scripts), `dialogs` (modals/settings), `thumbnails`, `file_watcher`, `validation` glue, `menu`, `layout`, `colors`, `textures` (SVG icon loaders).
- `src/copy/` — transfer engine: `plan` (walk+classify), `job` (worker threads), `engine` (BLAKE3 stream copy + verify).
- `src/validation/` — background asset checks (`root_entries`, `local_freshness`, `needs_review`).
- `src/{auth,notion,polyhaven,config,cache,slug,updater}.rs` — Auth0 login, admin API, public-API published-slug cache, config, JSON cache, slug parse/validate, self-update.

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

## Supporting subsystems
- **Validation** runs on a worker pool over visible assets, debounced after watcher events; results keyed by `RowKey` in `validation_results`. Checks: `root_entries` (expected `raw/staging/work`), `local_freshness` (local newer than prod when needs-review), `needs_review` (required staging files present for review statuses).
- **File watcher** is activity-aware: real-time `notify` watching when the window is active, polling (≈30s→idle) when not.
- **Thumbnails**: source priority Local → Prod → Archive (Archive only when status is Complete); cached by `(mtime,size)`, pruned >60 days.
- **Admin** (`AppState::is_admin`, from the JWT role claim): admin-only HDRI context-menu scripts `Normalize`/`Render` (Python under `~/Poly Haven Dropbox/Assets/PH Utils/Scripts/HDRIs`).
- **Auth**: Auth0 PKCE browser login, callback `127.0.0.1:45873`; tokens stored (unencrypted) in `config.toml`, auto-refreshed (`auth::ensure_access_token`, 60s buffer).
- **API**: `GET api/phase/assets?type=`, `PATCH api/phase/assets/{page_id}/status`, `…/title`. HTTP via `reqwest::blocking`.

## Conventions
- Commits: `feat:` / `fix:` / etc. prefix.
- Releases: bump `version` in `Cargo.toml`, create a **lightweight `v*` tag**, push the tag → `.github/workflows/release.yml` builds the Windows binary and publishes a GitHub Release (notes auto-generated from commits since the previous tag). The in-app `updater` (`self_update`) checks GitHub releases ~once/day. Commit/tag/push only when asked.
- **`Cargo.lock` is gitignored** (so is `/target` and `/docs/superpowers/`) — version bumps touch only `Cargo.toml`.
- Match surrounding style; new background work should follow the thread→channel→`pump()` pattern, and new per-asset settings go in `config.rs` + the Settings dialog (`dialogs::settings`).
