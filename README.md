# PHASE

![PHASE screenshot](https://u.polyhaven.org/c14/2026-06-02_06-45-03.png)

PHASE is a Windows desktop tool for managing Poly Haven asset folders between the production NAS and local working folders, while keeping the asset list and workflow status in sync with the admin system.

## Intent

PHASE is designed to make common asset-handling tasks faster and safer for the team: finding the right asset, seeing its current status, pulling it locally to work on it, pushing changes back to production, and spotting issues before they become pipeline problems.

## Primary workflow

1. Sign in through the browser.
2. Browse HDRIs and Textures, then filter by status or author.
3. Pull an asset from production to the local work folder.
4. Work on the asset and push changes back when ready.
5. Use the row status, warnings, and validation messages to catch issues and track progress.

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
