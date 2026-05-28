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

On first launch, paste your Notion integration token when prompted. It is saved to `%APPDATA%\phase\config.toml`. Logs are written to `%APPDATA%\phase\phase.log`.

## Test

```powershell
cargo test
```
