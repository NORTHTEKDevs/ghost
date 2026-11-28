# Ghost installer for Windows: the PowerShell counterpart of install.sh.
#
#   irm https://raw.githubusercontent.com/NORTHTEKDevs/ghost/main/scripts/install-windows.ps1 | iex
#
# Downloads the latest release zip, checks it against the release's .sha256
# file, installs ghost.exe, ghost-mcp.exe and ghost-http.exe, adds the folder to
# your user PATH, registers the MCP server with Claude Code when the `claude`
# CLI is installed, and runs `ghost doctor`. No administrator rights; nothing
# outside the install folder and your user PATH is changed.
#
# Options (run a saved copy: powershell -File install-windows.ps1 -NoRegister):
#   -Dir <path>    install somewhere else (default %LOCALAPPDATA%\Programs\ghost)
#   -NoPath        leave PATH alone
#   -NoRegister    skip the Claude Code registration
#   -NoDoctor      skip the closing `ghost doctor`
param(
    [string]$Dir = (Join-Path $env:LOCALAPPDATA 'Programs\ghost'),
    [switch]$NoPath,
    [switch]$NoRegister,
    [switch]$NoDoctor
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$url = 'https://github.com/NORTHTEKDevs/ghost/releases/latest/download'
$bins = 'ghost.exe', 'ghost-mcp.exe', 'ghost-http.exe'

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ghost-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host '==> Downloading the latest release'
    $zip = Join-Path $tmp 'ghost-windows-x64.zip'
    Invoke-WebRequest "$url/ghost-windows-x64.zip" -OutFile $zip -UseBasicParsing
    # Saved to a file: Windows PowerShell 5.1 hands back .Content as bytes for
    # this content type, not text.
    $sumFile = Join-Path $tmp 'ghost-windows-x64.zip.sha256'
    Invoke-WebRequest "$url/ghost-windows-x64.zip.sha256" -OutFile $sumFile -UseBasicParsing
    $want = ([IO.File]::ReadAllText($sumFile).Trim() -split '\s+')[0].ToLower()
    $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($want -ne $got) { throw "checksum mismatch: the release says $want, the download is $got" }
    Write-Host "==> Checksum OK ($got)"

    $unpacked = Join-Path $tmp 'unpacked'
    Expand-Archive $zip -DestinationPath $unpacked
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    foreach ($b in $bins) {
        $dst = Join-Path $Dir $b
        # A running MCP client keeps ghost-mcp.exe open; Windows lets a file in
        # use be renamed but not replaced, so move it aside first.
        if (Test-Path $dst) {
            try { Remove-Item $dst -Force } catch { Rename-Item $dst "$b.old-$stamp" }
        }
        Copy-Item (Join-Path $unpacked $b) $dst
        Unblock-File $dst
        Write-Host "==> installed $dst"
    }

    if (-not $NoPath) {
        $path = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (($path -split ';') -notcontains $Dir) {
            [Environment]::SetEnvironmentVariable('Path', (($path.TrimEnd(';'), $Dir) -join ';').TrimStart(';'), 'User')
            Write-Host "==> added $Dir to your user PATH (open a new terminal to use it)"
        }
    }

    $mcp = Join-Path $Dir 'ghost-mcp.exe'
    if (-not $NoRegister) {
        if (Get-Command claude -ErrorAction SilentlyContinue) {
            Write-Host '==> Registering the MCP server with Claude Code (user scope)'
            claude mcp remove ghost --scope user 2>$null | Out-Null
            claude mcp add ghost --scope user -- $mcp
        } else {
            Write-Host "claude CLI not found; for other clients the server is: $mcp"
        }
    }

    if (-not $NoDoctor) {
        Write-Host '==> Running diagnostics'
        & (Join-Path $Dir 'ghost.exe') doctor
    }
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
