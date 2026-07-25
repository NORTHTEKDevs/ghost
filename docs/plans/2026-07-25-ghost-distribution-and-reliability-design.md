# Ghost — Distribution + Reliability Design

Date: 2026-07-25
Status: approved, pending implementation plan

Two tracks run in parallel. The public "Buy" button stays disabled until the
Track A gate is green.

## Problem

`README.md:70` advertises a $20 ready-to-run kit at `northtek.io/ghost`. That
page does not exist, there are zero published releases, and there is no
packaging script. There is currently no way to obtain Ghost without a Rust
toolchain.

Separately, Ghost is reported as unreliable in live use: laggy, flaky, failing
on other machines, and leaving stuck processes.

## Evidence (measured 2026-07-25)

| Check | Result |
| --- | --- |
| `cargo test --workspace --release` | 399 passed, 0 failed, exit 0 |
| Real-Windows tests inside that run | all `#[ignore]`d — excluded |
| Live run (`-- --ignored`, 12 tests) | 11 passed, **1 failed** |
| `test_notepad_type_text` | `ElementNotFound { query: "role=edit" }` |
| 3 Edge browser tests | vacuous — ran in 0.02s, zero assertions |
| Region capture | GDI flat ~16.7ms; DXGI idle 1.74ms; DXGI 1600x900 83ms |
| `ghost-mcp.exe` processes | 7 across 6 live Claude sessions, all parents alive |

The four reported symptoms share one structural cause: **the test suite measures
logic, not behavior.** A green `cargo test` says nothing about whether Ghost can
drive a real application.

Two failure modes make the live suite unable to report problems:

```rust
// crates/ghost-session/tests/browser_flow.rs
let Some(s) = run_on("msedge.exe").await else { return; };  // missing app -> silent PASS
let _ = s.navigate_and_wait("Edge", &url, 10_000).await;     // result discarded, no assertion
```

### Root cause candidate — WinUI text surfaces

This machine runs `Microsoft.WindowsNotepad 11.2605.34.0`, the WinUI Store app.
`crates/ghost-core/src/uia/element.rs:128` maps only `50004 => "edit"`, while a
WinUI `RichEditBox` surfaces as Document (`50030`). `find(By::role("edit"))` is
exact-match and therefore misses modern WinUI text surfaces.

`crates/ghost-core/src/uia/tree.rs:50` already treats `["edit", "document"]` as
text roles, so the codebase is inconsistent with itself. Because every Windows 11
box ships these apps, this single gap plausibly explains "fails on other
machines".

Confidence: high, **unproven** until the red test goes green under the fix.

### Not established

- "Feels laggy" has no supporting evidence yet. Capture is 16.7ms and the
  recorded 2026-07-19 finding (cold boot 20-27ms, agent round-trips dominate)
  stands until disproved. Track A4 may conclude Ghost is not the bottleneck;
  that conclusion will be reported rather than replaced with a speculative
  optimization.
- Process hygiene looks correct. 7 processes for 6 sessions with all parents
  alive is not a runaway leak.

## Track A — Reliability

**A1. Test integrity (the root fix).** Replace the `#[ignore]` blanket with a
capability-gated harness. A test declares what it needs (`require_app!`,
`require_display!`); a missing capability produces a counted, printed SKIP and
never a silent pass. Every `let _ = ...` discard is deleted and replaced with an
assertion on an observable outcome.

GitHub-hosted runners have no interactive desktop, so the gate is local:
`scripts/verify-release.ps1` runs the full live matrix and must be green. The
packaging script calls it, so an unverified kit cannot be built.

**A2. WinUI role gap.** `By::role("edit")` becomes a role *class* covering text
surfaces (`edit` ∪ `document`), consistent with `tree.rs:50`. Exact-match
matching remains available. Verified against WinUI Notepad, a legacy Win32 edit
control, and the Edge address bar. The existing failing test is the red case.

**A3. Cross-machine — `ghost doctor`.** A preflight command reporting PASS/WARN
for Windows build, UIA availability, interactive session, DPI awareness, and
optional vision credentials. Includes a DPI-scaling audit, since non-DPI-aware
coordinates are a classic silent cross-machine break. This is also the primary
support-cost reducer for a paid kit.

**A4. Latency — measure first.** Add per-verb timing instrumentation to
`ghost-mcp`, then measure a real session. No optimization work begins before
that data exists.

## Track B — Distribution

**Artifact.** `scripts/package-kit.ps1` builds release binaries, runs the A1
gate, and emits `ghost-kit-vX.Y.Z-win-x64.zip` (three executables, quick-start,
`mcp-config.json`, examples) plus a SHA256.

**Hosting.** Vercel Blob, private. A static `/downloads/` path is publicly
guessable and would defeat the paywall.

**Flow.**

1. `ghost.html` — landing and purchase page, dark aurora identity per the site
   `DESIGN.md`.
2. `api/ghost/checkout.js` — Stripe Checkout, `mode: 'payment'` (one-time),
   dedicated `STRIPE_PRICE_GHOST_KIT`.
3. `api/ghost/webhook.js` — **raw body with `bodyParser: false`**, signature
   verified, issues `NTK-GHOST-xxxx` to KV and emails via Resend.
4. `api/ghost/download.js` — validates the license, rate-limited, capped at ~10
   downloads per license, returns a 5-minute signed Blob URL.
5. `ghost-activated.html` — license key, download button, SmartScreen guidance.

**README.** Option A rewritten to point at the real page, with an honest note
that binaries are unsigned.

### Known defect in the existing payment code (not fixed here)

`api/license/webhook.js:68` calls `stripe.webhooks.constructEvent` with
`JSON.stringify(req.body)` and does not disable Vercel's body parser. Stripe
signature verification requires the exact raw bytes, so re-serialized JSON will
not verify. If this is live, that webhook returns 400 on every event and never
issues keys. It belongs to a different product (realtor.northtek.io) and is out
of scope, but the pattern must not be copied and the Stripe delivery log should
be checked.

## Testing

- Red-green on the Notepad failure.
- Role-class unit tests plus a live text-entry matrix (WinUI, Win32, browser).
- Live gate green before packaging; packaging enforces it.
- Stripe **test mode** end-to-end via `live-flow-tester` before real keys.
- Post-deploy smoke against `northtek.io/ghost`.

## Decisions

- Both tracks run in parallel; the Buy button is gated on Track A passing.
- Binaries ship **unsigned**, with documented SmartScreen guidance. Signing
  (~$200-400/yr EV, or ~$10/mo Azure Trusted Signing) is deferred.

## Out of scope

macOS and Linux (scaffolds only, unverifiable on this machine), code signing,
and new verbs — the recorded constraint is that adoption, not surface area, is
Ghost's limiting factor.
