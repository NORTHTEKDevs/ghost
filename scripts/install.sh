#!/usr/bin/env bash
# Ghost installer for Linux.
#
# Builds the binaries, installs them to a user-writable prefix, registers the
# MCP server with Claude Code, and runs `ghost doctor` so a broken setup is
# reported here rather than in the middle of the first session.
#
#   ./scripts/install.sh                 # build + install + register + doctor
#   ./scripts/install.sh --no-register   # skip the Claude Code MCP registration
#   ./scripts/install.sh --prefix ~/bin  # install somewhere else
#
# Deliberately does not use sudo. Everything lands under $HOME.

set -euo pipefail

PREFIX="${HOME}/.local/bin"
REGISTER=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --no-register) REGISTER=0; shift ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's|^# \{0,1\}||'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

say()  { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# --------------------------------------------------------------- preflight

command -v cargo >/dev/null || die "cargo not found. Install Rust: https://rustup.rs"

# Runtime components. The engine itself is pure Rust, so nothing is needed at
# BUILD time; these are what the accessibility and portal paths talk to.
missing=()
[[ -e /usr/lib64/at-spi2-core/at-spi-bus-launcher || -e /usr/libexec/at-spi-bus-launcher || -e /usr/lib/at-spi2-core/at-spi-bus-launcher ]] \
  || missing+=("at-spi2-core")

if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]]; then
  command -v /usr/libexec/xdg-desktop-portal >/dev/null 2>&1 \
    || [[ -e /usr/libexec/xdg-desktop-portal ]] \
    || missing+=("xdg-desktop-portal xdg-desktop-portal-gnome")
fi

if [[ ${#missing[@]} -gt 0 ]]; then
  warn "missing runtime packages: ${missing[*]}"
  warn "install with: sudo dnf install ${missing[*]}"
fi

# --------------------------------------------------------------- build

say "Building release binaries"
cargo build --release --bin ghost --bin ghost-mcp --bin ghost-http

mkdir -p "$PREFIX"
for b in ghost ghost-mcp ghost-http; do
  install -m 0755 "target/release/$b" "$PREFIX/$b"
  say "installed $PREFIX/$b"
done

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) warn "$PREFIX is not on your PATH. Add it:"
     warn "  echo 'export PATH=\"$PREFIX:\$PATH\"' >> ~/.bashrc" ;;
esac

# --------------------------------------------------------------- accessibility

# The single most common reason Ghost "sees nothing" on Linux. Applications
# publish an accessible tree only when this is true, and they read it at
# startup - hence the restart note.
if command -v gsettings >/dev/null 2>&1; then
  current="$(gsettings get org.gnome.desktop.interface toolkit-accessibility 2>/dev/null || echo unknown)"
  if [[ "$current" == "false" ]]; then
    say "Enabling toolkit accessibility"
    gsettings set org.gnome.desktop.interface toolkit-accessibility true
    warn "restart any already-running applications so they publish a tree"
  fi
fi

# --------------------------------------------------------------- MCP registration

if [[ $REGISTER -eq 1 ]]; then
  if command -v claude >/dev/null 2>&1; then
    say "Registering the MCP server with Claude Code (user scope)"
    # Idempotent: remove any previous registration before adding.
    claude mcp remove ghost --scope user >/dev/null 2>&1 || true
    claude mcp add ghost --scope user -- "$PREFIX/ghost-mcp"
  else
    warn "claude CLI not found; register manually:"
    warn "  claude mcp add ghost --scope user -- $PREFIX/ghost-mcp"
  fi
fi

# --------------------------------------------------------------- verify

say "Running diagnostics"
set +e
"$PREFIX/ghost" doctor
rc=$?
set -e

echo
if [[ $rc -eq 0 ]]; then
  say "Ghost is installed and its checks pass."
else
  warn "ghost doctor reported a FAIL. See docs/linux-fedora.md section 6."
fi
exit $rc
