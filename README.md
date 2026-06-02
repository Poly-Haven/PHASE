# PHASE

Internal Windows desktop tool for managing asset files between the production NAS and local working folders.

## Build

Requires the Rust MSVC toolchain (install via [rustup](https://rustup.rs/)).

```powershell
cargo build --release
```

The executable is produced at `target\release\phase.exe`.

## Run

```powershell
.\target\release\phase.exe
```

On first launch, PHASE opens an Auth0 browser login and listens for the callback at `http://127.0.0.1:45873/callback`. PHASE stores Auth0 access/refresh tokens in `%APPDATA%\phase\config.toml`; it does not store a Notion API key.

Debug builds call the local admin backend at `http://localhost:3001/`. Release builds call `https://admin.polyhaven.com/`. Logs are written to `%APPDATA%\phase\phase.log`.

## Test

```powershell
cargo test
```
