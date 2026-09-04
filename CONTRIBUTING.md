# Contributing to Ghost

Ghost is a Rust workspace. The shared layers (`ghost-session`, `ghost-mcp`,
`ghost-cli`, `ghost-http`, `ghost-browser`) are written once; the engine under
them is `ghost-core` on Windows and `ghost-linux` on Linux, picked by a one-line
`cfg` alias in `ghost-session`. The engines mirror each other's module tree and
function signatures, so a function added to one engine needs a counterpart (or
an honest `Unsupported` error) in the other, or the other platform stops
compiling.

## Build and test

```bash
cargo build --workspace
cargo test --workspace                       # unit tests; live desktop tests are #[ignore]d
cargo clippy --workspace -- -D warnings      # CI fails on any warning
```

`rustfmt` is not enforced on this repo; match the style of the file you are in.

The unit suite proves compilation and pure logic only. Anything that touches a
real desktop is `#[ignore]`d and runs separately:

- **Windows:** `scripts/verify-release.ps1` runs the workspace suite, the ignored
  live suite and `ghost doctor` against the real desktop. It cannot run on a
  GitHub runner (no interactive desktop), so run it locally before a release.
  `scripts/live-on-hidden-desktop.ps1` runs the live suite on a hidden desktop so
  it never touches your screen.
- **Linux:** the `Linux` workflow synthesises a desktop on the runner (Xvfb,
  D-Bus, at-spi-bus-launcher) and drives a real GTK application, on both
  `ubuntu-latest` and a Fedora container. Locally: `cargo test -p ghost-linux
  -- --ignored` inside a desktop session with `at-spi2-core` running.

If you are on Windows and change shared code, check that Linux still compiles:
`cargo check --workspace --target x86_64-unknown-linux-gnu` (needs the target
installed and a C compiler for the target for `ring`; `zig cc` works).

## What a change needs

- A test that fails without it, where the change is observable without a live
  desktop. Live behaviour gets a live test in the ignored suite.
- No silent fallbacks. A verb that cannot do what was asked returns an error
  naming the action and what would unblock it; it never claims `ok: true` for
  something it could not confirm. Every action response carries `verified`.
- Nothing that takes the user's foreground under the default `background`
  policy. `ghost_stats.interference_audit` must stay at zero synthetic changes
  through the live suite.
- A CHANGELOG entry under the unreleased version.

## Releases

Releases are cut by pushing a `v*` tag. The `Release` workflow builds Windows
and Linux binaries, smoke-tests the Linux MCP server against its own protocol,
and publishes one GitHub release with both archives and their SHA-256 files.
Keep `crates/ghost-mcp/Cargo.toml`, `server.json` and the CHANGELOG heading on
the same version.

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.
