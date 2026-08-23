# BSHR Iterate Loop - ghost
#
# The deterministic core of the review -> certify -> debug loop. Every reflexion
# iteration must pass this gate before any agent-judged step runs; a RED gate is
# the loop's stop signal, not something to reason around.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\bshr-loop.ps1
# Exit:   0 = all gates green, 1 = a gate failed (see output)

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$fail = $false

function Gate($name, $scriptblock) {
    Write-Host "=== GATE: $name" -ForegroundColor Cyan
    & $scriptblock
    if ($LASTEXITCODE -ne 0) {
        Write-Host "=== FAIL: $name (exit $LASTEXITCODE)" -ForegroundColor Red
        $script:fail = $true
        return
    }
    Write-Host "=== PASS: $name" -ForegroundColor Green
}

# NOTE: no `cargo fmt --check` gate - this repo does not enforce rustfmt
# (a repo-wide `cargo fmt --all` produces a 6.5k-line diff). Add the gate
# only alongside a deliberate one-time formatting commit.

Gate "clippy"    { cargo clippy --workspace --quiet -- -D warnings }
Gate "tests"     { cargo test --workspace --quiet }
Gate "release"   { cargo build --release --bin ghost --bin ghost-http --bin ghost-mcp }

if (-not $fail) {
    # Live gates: drive the real binaries, not just the compiler.
    Gate "doctor"   { .\target\release\ghost.exe doctor }
    Gate "verify"   { .\target\release\ghost.exe verify }
    # Live isolated-desktop input contract (typing never lies about success).
    Gate "desktop-input-contract" { cargo test -p ghost-core --test desktop_input -- --ignored }
}

if ($fail) { exit 1 } else { Write-Host "`nALL GATES GREEN" -ForegroundColor Green; exit 0 }
