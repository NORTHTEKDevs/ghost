# Build ghost-mcp (release) and install it to the stable path every Claude
# session launches from, so new sessions pick up the current version.
#
#   powershell -ExecutionPolicy Bypass -File scripts/install.ps1
#
# Run this after changing Ghost. The scheduled task "GhostMcpAutoSync" also copies
# the latest build to the stable path automatically; this script is the explicit
# "rebuild + install now" command.
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
    cargo build --release -p ghost-mcp
    $dst = Join-Path $env:USERPROFILE '.local\bin\ghost-mcp.exe'
    New-Item -ItemType Directory -Force -Path (Split-Path $dst) | Out-Null
    # Reuse the sync engine — it handles the "stable binary is locked by a live
    # session" case (rename-aside then copy) that a plain Copy-Item can't.
    & (Join-Path $env:USERPROFILE '.local\bin\ghost-mcp-sync.ps1')
    Write-Host "Installed ghost-mcp -> $dst ($((Get-Item $dst).Length) bytes)"
    Write-Host "New Claude sessions will launch this build."
} finally {
    Pop-Location
}
