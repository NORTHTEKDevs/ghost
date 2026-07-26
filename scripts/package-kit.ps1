#requires -Version 5.1
<#
.SYNOPSIS
  Build the Ghost ready-to-run kit sold at northtek.io/ghost.

.DESCRIPTION
  Produces ghost-kit-v<version>-win-x64.zip plus a SHA256 sidecar.

  The gate is not optional. verify-release.ps1 runs the full live desktop suite
  and this script refuses to package if it fails, so an unverified binary cannot
  reach a paying customer. Use -SkipGate only for local packaging experiments;
  never for a build you intend to upload.

.EXAMPLE
  powershell -NoProfile -File scripts/package-kit.ps1
#>
[CmdletBinding()]
param(
    [switch]$SkipGate,
    [string]$OutDir,
    # Alternate binary directory. Lets the kit be built from an isolated
    # CARGO_TARGET_DIR when other Claude sessions hold a lock on
    # target/release/ghost-mcp.exe.
    [string]$BinDir
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$OutDir = if ($OutDir) { $OutDir } else { Join-Path $repo 'dist/out' }

# --- version comes from the shipped MCP binary, the headline component ---
$mcpToml = Join-Path $repo 'crates/ghost-mcp/Cargo.toml'
$version = (Select-String -Path $mcpToml -Pattern '^version\s*=\s*"(.+?)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw "could not read version from $mcpToml" }
Write-Host "Ghost kit v$version" -ForegroundColor Cyan

# --- 1. build ---
Write-Host "`n=== build (release) ===" -ForegroundColor Cyan
Push-Location $repo
try {
    # A running ghost-mcp.exe holds a lock and cargo silently skips the relink.
    $running = @(Get-Process ghost-mcp -ErrorAction SilentlyContinue)
    if ($running.Count -and -not $BinDir) {
        throw "$($running.Count) ghost-mcp.exe process(es) are running and hold a lock on the output binary; cargo would silently skip the relink and you would ship a stale exe. Stop them and re-run."
    }
    if ($BinDir) {
        Write-Host "using prebuilt binaries from $BinDir" -ForegroundColor DarkGray
    } else {
        cargo build --release --bin ghost --bin ghost-http --bin ghost-mcp
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    }
} finally { Pop-Location }

# --- 2. gate ---
if ($SkipGate) {
    Write-Warning 'GATE SKIPPED - this kit is NOT verified and must not be uploaded.'
} else {
    Write-Host "`n=== release gate ===" -ForegroundColor Cyan
    & (Join-Path $PSScriptRoot 'verify-release.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'release gate FAILED - refusing to package' }
}

# --- 3. stage ---
$stage = Join-Path $env:TEMP "ghost-kit-$version"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

$binaries = 'ghost.exe', 'ghost-http.exe', 'ghost-mcp.exe'
foreach ($b in $binaries) {
    $src = if ($BinDir) { Join-Path $BinDir $b } else { Join-Path $repo "target/release/$b" }
    if (-not (Test-Path $src)) { throw "missing binary: $src" }
    Copy-Item $src (Join-Path $stage $b)
}

foreach ($f in 'quick-start.md', 'mcp-config.json') {
    $src = Join-Path $repo "kit/$f"
    if (-not (Test-Path $src)) { throw "missing kit file: $src" }
    Copy-Item $src (Join-Path $stage $f)
}

Copy-Item (Join-Path $repo 'LICENSE') (Join-Path $stage 'LICENSE') -ErrorAction SilentlyContinue
$examplesSrc = Join-Path $repo 'examples'
if (Test-Path $examplesSrc) {
    Copy-Item $examplesSrc (Join-Path $stage 'examples') -Recurse
}

# --- 4. zip + checksum ---
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$zip = Join-Path $OutDir "ghost-kit-v$version-win-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $zip -Leaf)" | Set-Content -Path "$zip.sha256" -Encoding ascii
Remove-Item $stage -Recurse -Force

$sizeMb = [math]::Round((Get-Item $zip).Length / 1MB, 2)
Write-Host "`nPACKAGED" -ForegroundColor Green
Write-Host "  $zip"
Write-Host "  $sizeMb MB"
Write-Host "  sha256 $hash"
if ($SkipGate) { Write-Host "  UNVERIFIED (gate skipped) - do not upload" -ForegroundColor Yellow }
