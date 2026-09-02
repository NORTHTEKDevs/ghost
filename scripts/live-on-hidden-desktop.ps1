#requires -Version 5.1
<#
.SYNOPSIS
  Run a command on a hidden Windows desktop, so the live test suite never
  touches the screen, keyboard focus, or apps the user has open.
.DESCRIPTION
  The ignored live tests launch and drive real applications. Run on the user's
  desktop they steal focus for minutes and, before the testbed replaced Notepad,
  typed into the user's own files. A desktop object created here is never
  displayed and has its own input queue; a process started on it, and every
  process IT starts, lives there. UI Automation and window messages work as
  usual; real SendInput does not (Windows refuses it off the input desktop),
  which is why the live tests use Ghost's background paths.

  Default command: the ignored suite, serialized, release profile.
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/live-on-hidden-desktop.ps1
  powershell -ExecutionPolicy Bypass -File scripts/live-on-hidden-desktop.ps1 -Command "cargo test -p ghost-session --test testbed -- --ignored --nocapture"
#>
[CmdletBinding()]
param(
    [string]$Command = "cargo test --workspace --release --no-fail-fast -- --ignored --test-threads=1",
    [string]$LogPath = ""
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (-not $LogPath) { $LogPath = Join-Path $repo "target\live-hidden-desktop.log" }

Add-Type -Namespace Ghost -Name Desk -MemberDefinition @'
[DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
public static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr devmode, uint flags, uint access, IntPtr sa);
[DllImport("user32.dll", SetLastError=true)]
public static extern bool CloseDesktop(IntPtr h);
[StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
public struct STARTUPINFO { public int cb; public IntPtr lpReserved; public string lpDesktop; public IntPtr lpTitle; public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags; public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError; }
[StructLayout(LayoutKind.Sequential)]
public struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
[DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
public static extern bool CreateProcessW(string app, System.Text.StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
[DllImport("kernel32.dll", SetLastError=true)]
public static extern uint WaitForSingleObject(IntPtr h, uint ms);
[DllImport("kernel32.dll", SetLastError=true)]
public static extern bool GetExitCodeProcess(IntPtr h, out uint code);
[DllImport("kernel32.dll", SetLastError=true)]
public static extern bool CloseHandle(IntPtr h);
'@

$GENERIC_ALL = 0x10000000
$name = "ghost-live-$PID"
$desk = [Ghost.Desk]::CreateDesktopW($name, [IntPtr]::Zero, [IntPtr]::Zero, 0, $GENERIC_ALL, [IntPtr]::Zero)
if ($desk -eq [IntPtr]::Zero) { throw "CreateDesktop failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
Write-Host "hidden desktop '$name' created; running: $Command" -ForegroundColor Cyan
Write-Host "log: $LogPath" -ForegroundColor DarkGray

try {
    New-Item -ItemType Directory -Force -Path (Split-Path $LogPath) | Out-Null
    # cmd.exe carries the redirection, so the child needs no inherited handles.
    $cmdline = New-Object System.Text.StringBuilder ("cmd.exe /c " + $Command + " > `"$LogPath`" 2>&1")
    $si = New-Object Ghost.Desk+STARTUPINFO
    $si.cb = [Runtime.InteropServices.Marshal]::SizeOf($si)
    $si.lpDesktop = $name
    $pi = New-Object Ghost.Desk+PROCESS_INFORMATION
    # PowerShell marshals $null as an empty application name (error 123/3), so
    # cmd.exe is named explicitly.
    $ok = [Ghost.Desk]::CreateProcessW($env:ComSpec, $cmdline, [IntPtr]::Zero, [IntPtr]::Zero, $false, 0, [IntPtr]::Zero, $repo, [ref]$si, [ref]$pi)
    if (-not $ok) { throw "CreateProcess failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
    $null = [Ghost.Desk]::WaitForSingleObject($pi.hProcess, [uint32]::MaxValue)
    $code = 0
    $null = [Ghost.Desk]::GetExitCodeProcess($pi.hProcess, [ref]$code)
    $null = [Ghost.Desk]::CloseHandle($pi.hProcess); $null = [Ghost.Desk]::CloseHandle($pi.hThread)
    if (Test-Path $LogPath) { Get-Content $LogPath | Select-Object -Last 60 }
    Write-Host "exit code: $code" -ForegroundColor $(if ($code -eq 0) { 'Green' } else { 'Red' })
    exit [int]$code
} finally {
    $null = [Ghost.Desk]::CloseDesktop($desk)
}
