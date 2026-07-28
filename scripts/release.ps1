# Build, package, and (optionally) publish a GitHub release for the version in
# Cargo.toml. Releases are built and published **locally** with the GitHub CLI --
# a CI run took ~10 minutes to do what takes under a minute here, and the release
# notes are written by hand (see AGENTS.md) instead of being a dump of commit
# subjects.
#
# Usage:
#   pwsh scripts/release.ps1                          # test + build + package -> dist\
#   pwsh scripts/release.ps1 -Publish                 # also tag, push the tag, create the release
#   pwsh scripts/release.ps1 -Publish -NotesFile n.md # ...with hand-written notes
[CmdletBinding()]
param(
    [switch]$Publish,
    [string]$NotesFile,
    # Skip the clean-working-tree guard (for a dry run against local edits).
    [switch]$AllowDirty,
    [switch]$SkipTests
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot

# Version (and therefore the tag) comes from Cargo.toml -- the single source of
# truth. Bump it before cutting a release.
$version = ((Get-Content (Join-Path $root 'Cargo.toml')) |
    Select-String '^\s*version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
$tag = "v$version"

# The in-app updater (self_update, src/updater.rs) finds its download by
# matching this target triple in the asset name -- do not rename the zip.
$target = 'x86_64-pc-windows-msvc'
$distDir = Join-Path $root 'dist'
$stage = Join-Path $distDir "phase-$tag-$target"
$zip = Join-Path $distDir "phase-$tag-$target.zip"

Write-Host "Releasing PHASE $tag" -ForegroundColor Cyan

Push-Location $root
try {
    if ($Publish) {
        # A local build ships whatever is in the working tree, so make sure that
        # is exactly the committed, pushed state (CI got this for free).
        if (-not $AllowDirty -and (git status --porcelain)) {
            throw 'Working tree is dirty. Commit (or stash) first, or pass -AllowDirty.'
        }
        git fetch origin --quiet
        if ($LASTEXITCODE -ne 0) { throw 'git fetch origin failed' }
        $head = (git rev-parse HEAD).Trim()
        $onRemote = git branch --remotes --contains $head 2>$null
        if (-not $onRemote) {
            throw "HEAD ($($head.Substring(0,7))) is not pushed. Push the branch before publishing."
        }
    }

    if (-not $SkipTests) {
        cargo test --quiet
        if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
    }

    cargo build --release --quiet
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo build --release failed (if it failed at the link step with "Access is denied", PHASE is running -- stop it and retry).'
    }

    # Package: the zip holds phase.exe at its root, which is what the updater
    # expects to extract.
    $exe = Join-Path $root 'target\release\phase.exe'
    if (-not (Test-Path $exe)) { throw "phase.exe not found at $exe" }
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item $exe (Join-Path $stage 'phase.exe')
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

    $sizeMb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host "Packaged $zip ($sizeMb MB)" -ForegroundColor Green

    if (-not $Publish) {
        Write-Host 'Re-run with -Publish to tag and create the GitHub release.' -ForegroundColor Yellow
        return
    }

    # Tag (lightweight, matching the existing v* tags) and push it.
    if (-not (git tag --list $tag)) {
        git tag $tag
        if ($LASTEXITCODE -ne 0) { throw "git tag $tag failed" }
    }
    git push origin $tag
    if ($LASTEXITCODE -ne 0) { throw "git push origin $tag failed" }

    # Create the release with both assets: the zip (used by the in-app updater)
    # and the bare exe (for a manual download).
    $stagedExe = Join-Path $stage 'phase.exe'
    gh release view $tag *> $null
    if ($LASTEXITCODE -eq 0) {
        gh release upload $tag $zip $stagedExe --clobber
        if ($LASTEXITCODE -ne 0) { throw 'gh release upload failed' }
        Write-Host "Updated assets on existing release $tag" -ForegroundColor Green
    }
    else {
        if ($NotesFile) {
            if (-not (Test-Path $NotesFile)) { throw "Notes file not found: $NotesFile" }
            gh release create $tag $zip $stagedExe --title "PHASE $tag" --notes-file $NotesFile
        }
        else {
            gh release create $tag $zip $stagedExe --title "PHASE $tag" --notes 'Release notes pending.'
        }
        if ($LASTEXITCODE -ne 0) { throw 'gh release create failed' }
        Write-Host "Published $tag" -ForegroundColor Green
        if (-not $NotesFile) {
            Write-Host "Placeholder notes written -- replace with: gh release edit $tag --notes-file <file>" -ForegroundColor Yellow
        }
    }
}
finally {
    Pop-Location
}
