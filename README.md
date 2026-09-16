# PHASE

![PHASE screenshot](https://u.polyhaven.org/6kE/2026-06-05_12-14-04.png)

PHASE is a Windows desktop tool for managing Poly Haven asset folders between the production NAS and local working folders, while keeping the asset list and workflow status in sync with the admin system. It also ingests photos off camera memory cards onto the NAS, verified and ready to sort.

## Intent

PHASE is designed to make common asset-handling tasks faster and safer for the team: finding the right asset, seeing its current status, pulling it locally to work on it, pushing changes back to production, and spotting issues before they become pipeline problems.

## Primary workflow

1. Sign in through the browser.
2. Browse HDRIs and Textures, then filter by status or author.
3. Pull an asset from production to the local work folder.
4. Work on the asset and push changes back when ready.
5. Use the row status, warnings, and validation messages to catch issues and track progress.

## Card ingest

Insert a camera memory card and PHASE offers to ingest it. Every still image is copied to
`P:\_INGEST\{card}\`, mirroring the card's own folder layout. Each file is checked twice: its
BLAKE3 hash is compared against what actually landed on the NAS, and its RAW data is decoded to
prove the photo itself survived — the 64px thumbnail that comes out of that decode is what the
sort step will use later.

Where a camera wrote both a RAW and a JPEG of the same shot, only the RAW is taken. Re-running an
ingest is safe and near-instant: PHASE remembers what it already verified. When a card finishes, a
`phase_log.txt` is written to the card itself recording what was copied and whether the card is
safe to format.

# Development

## Build

Requires the Rust MSVC toolchain (install via [rustup](https://rustup.rs/)).

```powershell
cargo build
```

The executable is produced at `target\debug\phase.exe`.

## Run

```powershell
.\target\debug\phase.exe
```

On first launch, PHASE opens an Auth0 browser login and listens for the callback at `http://127.0.0.1:45873/callback`. PHASE stores Auth0 access/refresh tokens in `%APPDATA%\phase\config.toml`; it does not store a Notion API key.

Debug builds (default) call the local admin backend at `http://localhost:3001/`. Release builds (`cargo build --release`) call `https://admin.polyhaven.com/`. Logs are written to `%APPDATA%\phase\phase.log`.

## Test

```powershell
cargo test
```

# Licence

PHASE is free software under the [GNU General Public License v3.0 or later](LICENSE). See
[NOTICE](NOTICE) for third-party components, notably [rawler](https://github.com/dnglab/dnglab)
(LGPL-2.1), which decodes camera RAW files during card ingest.
