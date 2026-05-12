# scripts/ship.ps1 — tag, push, and BLOCK until the MSI is downloadable.
#
# The CI workflow at .github/workflows/release.yml takes ~12 minutes
# to build the WiX MSI on a `git push --tags` for `v*`. The release
# stub is created early in the run, but `publishedAt` doesn't get
# populated until the MSI is uploaded and the draft is flipped to
# published.
#
# Pre-v0.4.23, "ship" meant `git push --tags` and a vibe. The window
# between push and "MSI is actually downloadable" was opaque — the
# in-app updater would show stale data for 5 minutes, and there was
# no signal that the publish path was healthy.
#
# This script closes that gap:
#   1. Verifies the working tree is clean.
#   2. Pushes main + the requested tag.
#   3. Polls `gh release view <tag>` every 15 s until `publishedAt`
#      becomes a real ISO timestamp.
#   4. Emits the asset SHA-256 fingerprints + the download URL so
#      the operator can verify what landed.
#
# Usage:
#   .\scripts\ship.ps1 -Tag v0.4.23
#
# Requires: gh CLI authenticated for ophiocus/TinyBoothSoundStudio.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Tag,

    # Skip the working-tree-clean check. Use sparingly.
    [switch]$AllowDirty,

    # How long to wait between polls of the GitHub release endpoint.
    # Default 15 s — fast enough to feel snappy, slow enough not to
    # rate-limit the GitHub API.
    [int]$PollSeconds = 15,

    # Hard ceiling so a stuck CI run doesn't hang the script forever.
    # Default 30 min — generous over the documented 12-min build.
    [int]$TimeoutSeconds = 1800
)

$ErrorActionPreference = 'Stop'

# ── 1. Sanity checks ────────────────────────────────────────────────
if (-not (Test-Path '.git')) {
    throw 'Run from the repo root (no .git/ found).'
}

if (-not $AllowDirty) {
    $status = git status --porcelain
    if ($status) {
        throw "Working tree is dirty. Commit, stash, or pass -AllowDirty.`n$status"
    }
}

# Confirm the tag matches a real annotated tag in the local repo.
$tagExists = git tag --list $Tag
if (-not $tagExists) {
    throw "Tag '$Tag' not found locally. Create it first (git tag $Tag) then re-run."
}

# Confirm gh is authenticated.
try {
    gh auth status 2>&1 | Out-Null
} catch {
    throw 'gh CLI is not authenticated. Run `gh auth login` first.'
}

# ── 2. Push ─────────────────────────────────────────────────────────
Write-Host "→ pushing main + $Tag" -ForegroundColor Cyan
git push origin main
git push origin $Tag

# ── 3. Poll until published ────────────────────────────────────────
Write-Host "→ waiting for CI to publish the release…" -ForegroundColor Cyan
$started = Get-Date
$deadline = $started.AddSeconds($TimeoutSeconds)

while ($true) {
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    if ((Get-Date) -gt $deadline) {
        throw "Timed out after ${TimeoutSeconds}s waiting for $Tag to be published. Check: gh run list --workflow=release.yml --limit 3"
    }

    # v0.4.24 — the JSON field list must be a single token with no
    # whitespace. PowerShell would otherwise parse `--json publishedAt,
    # assets` as TWO arguments (`publishedAt,` and `assets`), and gh
    # rejects the second one with `Unknown JSON field: " assets"`.
    # That silently failed the v0.4.23 poll for 14+ minutes.
    $json = $null
    $raw = (gh release view $Tag --json 'publishedAt,assets' 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -eq 0 -and $raw) {
        try {
            $json = $raw | ConvertFrom-Json
        } catch {
            $json = $null
        }
    }

    $pubAt = if ($json) { $json.publishedAt } else { $null }
    $assetCount = if ($json -and $json.assets) { @($json.assets).Count } else { 0 }

    # `$pubAt` is either `$null` (release stub not published yet) or a
    # `[DateTime]` parsed by ConvertFrom-Json from the ISO string the
    # API returned. PowerShell 7's auto-parse means a regex match
    # against `^20\d\d-` would miss every published release (the
    # DateTime's ToString is `MM/dd/yyyy …` in US locales). Just test
    # for non-null instead.
    if ($pubAt) {
        Write-Host "  ✓ published at $pubAt (${assetCount} asset(s)) after ${elapsed}s" -ForegroundColor Green
        break
    }

    Write-Host "  …${elapsed}s elapsed — publishedAt=$pubAt assets=$assetCount" -ForegroundColor DarkGray
    Start-Sleep -Seconds $PollSeconds
}

# ── 4. Report ──────────────────────────────────────────────────────
Write-Host ''
Write-Host '─── release artifacts ──────────────────────' -ForegroundColor Cyan
$final = gh release view $Tag --json 'name,publishedAt,url,assets' | ConvertFrom-Json
Write-Host ('  name       : {0}' -f $final.name)
Write-Host ('  published  : {0}' -f $final.publishedAt)
Write-Host ('  url        : {0}' -f $final.url)
foreach ($a in $final.assets) {
    $sizeMib = [math]::Round($a.size / 1MB, 2)
    Write-Host ''
    Write-Host ('  asset      : {0}' -f $a.name) -ForegroundColor White
    Write-Host ('    size     : {0} MiB' -f $sizeMib)
    Write-Host ('    sha256   : {0}' -f $a.digest.Replace('sha256:', ''))
    Write-Host ('    download : {0}' -f $a.url)
}
Write-Host ''
Write-Host '✓ Ship complete. The in-app updater will pick this up on its' -ForegroundColor Green
Write-Host '  next 5-min recheck (or immediately on tab switch — v0.4.23).' -ForegroundColor Green
