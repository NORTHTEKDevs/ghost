#requires -Version 5.1
<#
.SYNOPSIS
  Install Ghost and wire it into Claude. One command, no toolchain.

.DESCRIPTION
  Copies the binaries somewhere permanent, clears the browser's block flag,
  adds them to PATH, writes the MCP config for Claude Code and/or Claude
  Desktop, and runs `ghost doctor` so you know it works before you rely on it.

  Safe to re-run. It never overwrites an existing Claude config wholesale - it
  merges the `ghost` server entry and leaves everything else alone.

.PARAMETER InstallDir
  Where the binaries go. Default: %LOCALAPPDATA%\Programs\ghost

.PARAMETER SkipPath
  Do not modify the user PATH.

.PARAMETER SkipClaude
  Do not touch any Claude configuration.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\ghost'),
    [switch]$SkipPath,
    [switch]$SkipClaude
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$binaries = 'ghost.exe', 'ghost-http.exe', 'ghost-mcp.exe'

function Say($msg, $colour = 'Gray') { Write-Host $msg -ForegroundColor $colour }

Say "`nGhost installer" Cyan
Say "----------------------------------------"

# --- 0. sanity ---
foreach ($b in $binaries) {
    if (-not (Test-Path (Join-Path $here $b))) {
        throw "$b is not next to this script. Run install.ps1 from the folder you unzipped."
    }
}

# --- 1. unblock ---
# Files that came from a browser carry a mark-of-the-web that makes Windows
# refuse or warn on every launch. Clearing it here is why you only see the
# SmartScreen prompt once (if at all) instead of every time.
Get-ChildItem -Path $here -Recurse -File | Unblock-File -ErrorAction SilentlyContinue
Say "unblocked downloaded files" DarkGray

# --- 2. copy ---
if (-not (Test-Path $InstallDir)) { New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null }

# Only stop processes running FROM the directory we are about to overwrite.
# Killing every ghost-mcp on the machine would take down MCP servers belonging
# to unrelated Claude sessions, and any automation the user has running - a
# fresh install has no business doing that.
$target = (Resolve-Path $InstallDir).Path
$occupying = @(
    Get-Process ghost-mcp, ghost-http, ghost -ErrorAction SilentlyContinue |
        Where-Object {
            try { $_.Path -and (Split-Path $_.Path -Parent) -eq $target } catch { $false }
        }
)
if ($occupying.Count) {
    Say "stopping $($occupying.Count) Ghost process(es) running from $target" DarkGray
    $occupying | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 700
} else {
    $elsewhere = @(Get-Process ghost-mcp -ErrorAction SilentlyContinue).Count
    if ($elsewhere) {
        Say "note: $elsewhere Ghost process(es) are running from other locations - left alone" DarkGray
    }
}

foreach ($b in $binaries) { Copy-Item (Join-Path $here $b) (Join-Path $InstallDir $b) -Force }
foreach ($extra in 'quick-start.md', 'mcp-config.json') {
    if (Test-Path (Join-Path $here $extra)) { Copy-Item (Join-Path $here $extra) $InstallDir -Force }
}
foreach ($dir in 'examples', 'recipes') {
    if (Test-Path (Join-Path $here $dir)) {
        Copy-Item (Join-Path $here $dir) (Join-Path $InstallDir $dir) -Recurse -Force
    }
}
Say "installed to $InstallDir" Green

# --- 3. PATH ---
if (-not $SkipPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$InstallDir", 'User')
        Say "added to PATH (open a new terminal to pick it up)" Green
    } else {
        Say "already on PATH" DarkGray
    }
}

# --- 4. Claude wiring ---
$mcpPath = Join-Path $InstallDir 'ghost-mcp.exe'

function Merge-ClaudeDesktopConfig($configPath) {
    $dir = Split-Path $configPath -Parent
    if (-not (Test-Path $dir)) { return $false }

    $config = if (Test-Path $configPath) {
        try { Get-Content $configPath -Raw | ConvertFrom-Json } catch {
            Say "  existing config at $configPath is not valid JSON - leaving it alone" Yellow
            return $false
        }
    } else { [pscustomobject]@{} }

    if (-not $config.PSObject.Properties['mcpServers']) {
        $config | Add-Member -NotePropertyName mcpServers -NotePropertyValue ([pscustomobject]@{})
    }
    # Replace only our own entry; every other server the user has stays put.
    $entry = [pscustomobject]@{ command = $mcpPath; args = @() }
    if ($config.mcpServers.PSObject.Properties['ghost']) {
        $config.mcpServers.ghost = $entry
    } else {
        $config.mcpServers | Add-Member -NotePropertyName ghost -NotePropertyValue $entry
    }

    if (Test-Path $configPath) {
        Copy-Item $configPath "$configPath.bak" -Force   # never edit a config without a backup
    }
    $config | ConvertTo-Json -Depth 12 | Set-Content $configPath -Encoding UTF8
    return $true
}

if (-not $SkipClaude) {
    Say "`nwiring Claude" Cyan

    $desktop = Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'
    if (Merge-ClaudeDesktopConfig $desktop) {
        Say "  Claude Desktop: configured (restart it)" Green
    } else {
        Say "  Claude Desktop: not installed, skipped" DarkGray
    }

    $claudeCli = Get-Command claude -ErrorAction SilentlyContinue
    if ($claudeCli) {
        & claude mcp add ghost --scope user -- $mcpPath 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { Say "  Claude Code: registered at user scope" Green }
        else { Say "  Claude Code: run manually -> claude mcp add ghost --scope user -- `"$mcpPath`"" Yellow }
    } else {
        Say "  Claude Code CLI not found, skipped" DarkGray
    }
}

# --- 5. prove it ---
Say "`nchecking this machine" Cyan
& (Join-Path $InstallDir 'ghost.exe') doctor
$doctor = $LASTEXITCODE

Say "`n----------------------------------------"
if ($doctor -eq 0) {
    Say "Ghost is installed and this machine passes every required check." Green
    Say "Try it:  ghost launch notepad.exe" DarkGray
    Say "Recipes: $InstallDir\recipes" DarkGray
} else {
    Say "Installed, but ghost doctor reported a FAIL above." Yellow
    Say "Send us that output and we will tell you exactly what is wrong: info@northtek.io" Yellow
}
exit $doctor
