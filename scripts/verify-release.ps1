#requires -Version 5.1
<#
.SYNOPSIS
  Mandatory pre-release gate. package-kit.ps1 calls this and refuses to build
  a kit if it fails.

.DESCRIPTION
  Exists because `cargo test --workspace` is NOT evidence that Ghost works.
  Every test that drives real Windows is #[ignore]d and therefore excluded from
  the default run - on 2026-07-25 that default run was 399 green while the most
  basic operation in the product was broken. This gate runs the ignored suite
  too.

  GitHub-hosted runners have no interactive desktop, so this cannot live in CI.
  It runs on a real machine, before packaging, or it does not run at all.

  Live tests drive the shared desktop and must not run concurrently, hence
  --test-threads=1.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Continue'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
$failed = @()

try {
    Write-Host "`n=== 1/3 workspace suite ===" -ForegroundColor Cyan
    cargo test --workspace --release
    if ($LASTEXITCODE -ne 0) { $failed += 'workspace suite' }

    Write-Host "`n=== 2/3 live desktop suite ===" -ForegroundColor Cyan
    Write-Host 'Drives real windows. Do not touch the mouse or keyboard.' -ForegroundColor DarkGray
    $before = @(Get-Process Notepad -ErrorAction SilentlyContinue).Id
    cargo test --workspace --release --no-fail-fast -- --ignored --test-threads=1
    if ($LASTEXITCODE -ne 0) { $failed += 'live desktop suite' }

    # Some live tests drive WinUI apps, and killing the pid returned by launch()
    # does not stop a Store app - it kills the launcher stub. Reap only what this
    # run started, never a window the user had open.
    $leaked = @(Get-Process Notepad -ErrorAction SilentlyContinue | Where-Object { $_.Id -notin $before })
    if ($leaked.Count) {
        Write-Host "reaping $($leaked.Count) leaked Notepad process(es) from the live suite" -ForegroundColor DarkGray
        $leaked | Stop-Process -Force -ErrorAction SilentlyContinue
    }

    Write-Host "`n=== 3/3 ghost doctor ===" -ForegroundColor Cyan
    $doctor = Join-Path $repo 'target/release/ghost.exe'
    if (Test-Path $doctor) {
        & $doctor doctor
        if ($LASTEXITCODE -ne 0) { $failed += 'ghost doctor' }
    } else {
        $failed += 'ghost doctor (binary not built)'
    }
} finally {
    Pop-Location
}

Write-Host ''
if ($failed.Count) {
    Write-Host "GATE FAILED: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host 'GATE GREEN - safe to package' -ForegroundColor Green
exit 0
