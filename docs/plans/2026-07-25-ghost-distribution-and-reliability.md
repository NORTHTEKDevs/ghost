# Ghost Distribution + Reliability Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Ghost verifiably reliable on any Windows 11 machine, and make it purchasable and downloadable from northtek.io/ghost as a $20 one-time kit.

**Architecture:** Two parallel tracks. Track A (ghost repo, Rust) fixes the reason a green test suite coexists with a broken product: real-Windows tests are `#[ignore]`d and the browser tests cannot fail. Track B (northtek-site, static HTML + Vercel functions) adds a Stripe one-time checkout issuing a license that unlocks a signed, expiring Vercel Blob URL. The public Buy button stays disabled until Track A's gate script is green.

**Tech Stack:** Rust (9-crate workspace, tokio, windows-rs/UIA), PowerShell (packaging + gate), static HTML + Tailwind v3, Vercel serverless functions, Stripe, Vercel KV, Vercel Blob, Resend.

**Design doc:** `docs/plans/2026-07-25-ghost-distribution-and-reliability-design.md` (commit 69a1ad2)

---

## Ground truth measured 2026-07-25 (do not re-derive)

- `cargo test --workspace --release` → **399 passed, 0 failed**, exit 0.
- `cargo test --release -p ghost-session --no-fail-fast -- --ignored --test-threads=1` → **11 passed, 1 FAILED**.
- The failure: `test_notepad_type_text`, `ElementNotFound { query: "role=edit" }`.
- This machine: `Microsoft.WindowsNotepad 11.2605.34.0` (WinUI Store app).
- `crates/ghost-core/src/uia/tree.rs:21` `role_alias_matches` covers only `tab→tabitem` and `list→listitem`. **No `edit→document` alias.**
- `crates/ghost-core/src/uia/element.rs:128` maps `50004 => "edit"`, `50030 => "document"`.
- Region capture: GDI flat ~16.7ms all sizes; DXGI idle 1.74ms; DXGI 1600x900 = 83ms. Routing through GDI is already correct — **do not reintroduce DXGI region capture.**
- Cold boot ~20-27ms, RSS ~13MB (measured 2026-07-19). **Boot is not a bottleneck. Do not optimize it.**

## Rules for every task

- Live tests drive the shared desktop. **Always** run them with `--test-threads=1`.
- Never claim a task done without pasting the command output that proves it.
- `cargo build --release` silently skips relink while `ghost-mcp.exe` is running. If a build seems stale, stop the process and `touch crates/ghost-mcp/src/main.rs`.
- Commit messages: repo is PUBLIC. Author is `Northtek <info@northtek.io>` (already configured). **No Claude co-author trailer.**
- Foreground bash sometimes times out at 2m on this box (Defender). Use background execution with output redirected to a file for anything long.
- Do NOT launch `notepad.exe` for ad-hoc experiments — it attaches to the user's existing Notepad window containing `paper.md`. The tests kill the pid they launch; ad-hoc commands do not.

---

# TRACK A — RELIABILITY (repo: `~/projects/active/ghost`)

## Task 1: Fix the WinUI text-surface role gap

The red test already exists and fails. This is the whole bug.

**Files:**
- Modify: `crates/ghost-core/src/uia/tree.rs:21-27`
- Test: `crates/ghost-core/src/uia/tree.rs` (unit, in-file `mod tests`) + existing `crates/ghost-session/tests/notepad.rs`

**Step 1: Confirm the red test fails for the stated reason**

Run:
```bash
cd ~/projects/active/ghost
cargo test --release -p ghost-session --test notepad -- --ignored --test-threads=1 2>&1 | tail -20
```
Expected: `test_notepad_type_text ... FAILED` with `ElementNotFound { query: "role=edit" }`.

If it passes, STOP — the environment differs from the measurement and this plan's premise needs rechecking.

**Step 2: Write the failing unit test**

Add to the `mod tests` block in `crates/ghost-core/src/uia/tree.rs`:

```rust
#[test]
fn edit_role_matches_winui_document_surface() {
    // Win11 WinUI apps (Notepad, WordPad successor) expose their text area as
    // Document (50030), not Edit (50004). A caller asking for "edit" means
    // "the text input", so document must match.
    assert!(role_alias_matches("edit", "document"));
}

#[test]
fn document_role_does_not_reverse_match_edit() {
    // Asking specifically for a document should not return a plain textbox.
    assert!(!role_alias_matches("document", "edit"));
}

#[test]
fn existing_role_aliases_still_hold() {
    assert!(role_alias_matches("tab", "tabitem"));
    assert!(role_alias_matches("list", "listitem"));
    assert!(!role_alias_matches("button", "edit"));
}
```

**Step 3: Run it to verify it fails**

Run: `cargo test --release -p ghost-core role_matches -- --nocapture`
Expected: FAIL on `edit_role_matches_winui_document_surface`.

**Step 4: Implement the minimal fix**

Replace `crates/ghost-core/src/uia/tree.rs:21-27` with:

```rust
pub(crate) fn role_alias_matches(searched: &str, el_role: &str) -> bool {
    match searched {
        "tab" => el_role == "tabitem",
        "list" => el_role == "listitem",
        // Win11 WinUI text surfaces (Notepad, Mail, Store apps) report
        // Document (50030) rather than Edit (50004). Callers asking for
        // "edit" mean "the text input", so accept both. Deliberately
        // one-way: an explicit "document" search must not return a textbox.
        "edit" => el_role == "document",
        _ => false,
    }
}
```

**Step 5: Verify unit tests pass**

Run: `cargo test --release -p ghost-core role -- --nocapture`
Expected: all 3 new tests PASS.

**Step 6: Verify the live test goes green**

Run:
```bash
cargo test --release -p ghost-session --test notepad -- --ignored --test-threads=1 2>&1 | tail -10
```
Expected: `test result: ok. 2 passed; 0 failed`.

**Step 7: Verify nothing regressed**

Run: `cargo test --workspace --release 2>&1 | grep -E "test result:|error" | tail -20`
Expected: 0 failed, count >= 399+3.

**Step 8: Commit**

```bash
git add crates/ghost-core/src/uia/tree.rs
git commit -m "fix(uia): match WinUI Document surfaces when searching role=edit

Win11 ships Notepad and friends as WinUI apps whose text area reports
control type Document (50030), not Edit (50004). find(By::role(\"edit\"))
was exact-match plus a two-entry alias table that omitted this case, so
the most basic operation - launch an app and type into it - failed on
every stock Windows 11 machine.

Alias is one-way: an explicit document search still will not match a
plain textbox.

test_notepad_type_text: FAILED -> ok."
```

---

## Task 2: Prove the fix across all three text-surface families

One passing Notepad test is not proof the class is fixed.

**Files:**
- Create: `crates/ghost-session/tests/text_surface_matrix.rs`

**Step 1: Write the matrix test**

```rust
//! Verifies role=edit resolves across all three Windows text-surface families:
//! WinUI (Document), legacy Win32 (Edit), and Chromium (Edit in a DOM tree).
//! Run with: cargo test -p ghost-session --test text_surface_matrix -- --ignored --test-threads=1

#![cfg(windows)]

use ghost_session::{GhostSession, By};
use std::time::Duration;

async fn type_into(exe: &str, text: &str) -> Result<String, String> {
    let session = GhostSession::new().map_err(|e| e.to_string())?.with_timeout(8000);
    let pid = session.launch(exe).await.map_err(|e| format!("launch {exe}: {e}"))?;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let result = (|| async {
        let edit = session.find(By::role("edit")).await
            .map_err(|e| format!("find edit in {exe}: {e}"))?;
        edit.type_text(text).map_err(|e| format!("type into {exe}: {e}"))?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let readback = session.find(By::role("edit")).await
            .map_err(|e| format!("re-find edit in {exe}: {e}"))?;
        Ok::<String, String>(readback.value().unwrap_or_default())
    })().await;

    ghost_core::process::kill(pid).ok();
    result
}

#[tokio::test]
#[ignore]
async fn winui_notepad_accepts_typed_text() {
    let got = type_into("notepad.exe", "ghost-winui").await.expect("winui path");
    assert!(got.contains("ghost-winui"), "read back {got:?}");
}

#[tokio::test]
#[ignore]
async fn win32_legacy_edit_accepts_typed_text() {
    // mspaint's text tool and regedit's address bar are legacy Win32 Edit
    // controls. wordpad.exe was removed in Win11 24H2, so do not use it.
    let got = type_into("regedit.exe", "HKEY_CURRENT_USER").await.expect("win32 path");
    assert!(got.contains("HKEY"), "read back {got:?}");
}

#[tokio::test]
#[ignore]
async fn chromium_omnibox_accepts_typed_text() {
    let got = type_into("msedge.exe", "example.com").await.expect("chromium path");
    assert!(got.contains("example.com"), "read back {got:?}");
}
```

**Step 2: Run it**

Run: `cargo test --release -p ghost-session --test text_surface_matrix -- --ignored --test-threads=1 --nocapture`

**Step 3: Triage honestly**

Any failure here is a real gap in the fix, not a bad test. Fix the code, or if a family genuinely cannot work, document why in the test with `#[ignore = "reason"]` and record it in the design doc. **Do not** delete an assertion to get green — that is the exact defect Task 3 removes.

Note: `regedit.exe` may prompt UAC. If it does on this machine, substitute another legacy Win32 app with an edit control and note the substitution in the commit message.

**Step 4: Commit**

```bash
git add crates/ghost-session/tests/text_surface_matrix.rs
git commit -m "test(uia): live matrix covering WinUI, Win32 and Chromium text surfaces"
```

---

## Task 3: Delete the vacuous browser tests

These three tests are structurally incapable of failing: they `return` silently when Edge is absent and discard every result with `let _ =`. They ran in 0.02s and reported PASS.

**Files:**
- Modify: `crates/ghost-session/tests/browser_flow.rs`

**Step 1: Prove they are vacuous**

Run: `cargo test --release -p ghost-session --test browser_flow -- --ignored --test-threads=1 --nocapture 2>&1 | tail -8`
Expected: 3 passed in well under 1s — impossible for real browser automation.

**Step 2: Rewrite with real assertions**

Replace the three test bodies. `run_on` must fail loudly rather than skip silently (Task 4 introduces the proper skip mechanism; until then, a missing browser is a hard failure on the release machine):

```rust
async fn run_on(exe: &str) -> GhostSession {
    let s = GhostSession::new().expect("session").with_timeout(8000);
    let pid = s.launch(exe).await
        .unwrap_or_else(|e| panic!("{exe} must be installed on the release machine: {e}"));
    assert_ne!(pid, 0, "{exe} launch returned pid 0");
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    s
}

#[tokio::test]
#[ignore]
async fn navigate_and_wait_resolves_on_edge() {
    let s = run_on("msedge.exe").await;
    let url = fixture_url();
    let outcome = s.navigate_and_wait("Edge", &url, 10_000).await;
    assert!(outcome.is_ok(), "navigate_and_wait failed: {:?}", outcome.err());
}

#[tokio::test]
#[ignore]
async fn execute_intent_form_login_on_edge() {
    let s = run_on("msedge.exe").await;
    let url = fixture_url();
    let intent = format!(r#"{{"steps":[
        {{"op":"navigate","url":"{url}"}},
        {{"op":"wait_for_idle","timeout_ms":3000}}
    ],"max_duration_ms":15000}}"#);
    let report = s.execute_intent(&intent).await
        .expect("execute_intent returned Err");
    assert!(report.steps_completed >= 2,
        "expected both steps to complete, got {report:?}");
}
```

Adjust the final assertion to whatever `execute_intent` actually returns — read the type first, do not guess. Leave `describe_delta_small_payload_on_dom_change` as-is; it already asserts.

**Step 3: Run and confirm they now take real time**

Run: `cargo test --release -p ghost-session --test browser_flow -- --ignored --test-threads=1 --nocapture 2>&1 | tail -8`
Expected: PASS, and elapsed time in seconds, not 0.02s. If still instant, they are still vacuous.

**Step 4: Commit**

```bash
git add crates/ghost-session/tests/browser_flow.rs
git commit -m "test(browser): make Edge flow tests capable of failing

These three tests returned early when Edge was absent and discarded every
result with 'let _ =', so they asserted nothing and completed in 0.02s
while reporting PASS. Now they launch, assert on the returned outcome,
and fail loudly if the browser is missing."
```

---

## Task 4: Capability-gated skips that cannot be mistaken for passes

**Files:**
- Create: `crates/ghost-session/tests/common/mod.rs`
- Modify: the live test files to use it

**Step 1: Write the helper**

```rust
//! Live tests need a real desktop and specific apps. A missing capability
//! must be a LOUD, COUNTED skip - never a silent pass, which is how the
//! browser suite reported green while asserting nothing.

use std::path::Path;

pub fn app_available(exe: &str) -> bool {
    if Path::new(exe).is_absolute() { return Path::new(exe).exists(); }
    std::env::var("PATH").ok().is_some_and(|p| {
        std::env::split_paths(&p).any(|d| d.join(exe).exists())
    }) || Path::new(r"C:\Windows\System32").join(exe).exists()
}

pub fn has_interactive_desktop() -> bool {
    // Session 0 / service context has no interactive desktop; UIA silently
    // returns empty trees there instead of erroring.
    unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() != 0xFFFF_FFFF }
}

/// Skip the test loudly. Set GHOST_TEST_STRICT=1 (the release gate does) to
/// turn every skip into a failure, so packaging cannot silently ship untested.
#[macro_export]
macro_rules! require_app {
    ($exe:expr) => {
        if !$crate::common::app_available($exe) {
            if std::env::var("GHOST_TEST_STRICT").is_ok() {
                panic!("STRICT: required app {} missing", $exe);
            }
            eprintln!("SKIP[{}]: {} not installed", module_path!(), $exe);
            return;
        }
    };
}
```

`GHOST_TEST_STRICT=1` is the load-bearing part: on a developer laptop a missing browser skips, but the release gate sets it, so a skip becomes a hard failure and an untested kit cannot be packaged.

**Step 2: Wire it into `browser_flow.rs` and `text_surface_matrix.rs`**, replacing the panic in `run_on` with `require_app!("msedge.exe")` at the top of each test.

**Step 3: Verify both modes**

```bash
cargo test --release -p ghost-session --test browser_flow -- --ignored --test-threads=1 2>&1 | tail -5
GHOST_TEST_STRICT=1 cargo test --release -p ghost-session --test browser_flow -- --ignored --test-threads=1 2>&1 | tail -5
```
Expected: normal run passes or prints `SKIP[...]`; strict run fails hard if any app is missing.

**Step 4: Commit**

```bash
git add crates/ghost-session/tests/common/mod.rs crates/ghost-session/tests/
git commit -m "test: counted capability skips with GHOST_TEST_STRICT for the release gate"
```

---

## Task 5: The release gate script

Nothing ships without this being green. GitHub-hosted runners have no interactive desktop, so this gate is local and mandatory.

**Files:**
- Create: `scripts/verify-release.ps1`

**Step 1: Write it**

```powershell
#requires -Version 5.1
# Mandatory pre-release gate. package-kit.ps1 calls this and refuses to build
# a kit if it fails. Live tests drive the shared desktop, so they run serially.
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
$failed = @()

Write-Host "`n=== 1/3 unit + integration suite ===" -ForegroundColor Cyan
cargo test --workspace --release
if ($LASTEXITCODE -ne 0) { $failed += 'workspace suite' }

Write-Host "`n=== 2/3 live desktop suite (strict) ===" -ForegroundColor Cyan
$env:GHOST_TEST_STRICT = '1'
cargo test --workspace --release --no-fail-fast -- --ignored --test-threads=1
if ($LASTEXITCODE -ne 0) { $failed += 'live desktop suite' }
Remove-Item Env:\GHOST_TEST_STRICT

Write-Host "`n=== 3/3 ghost doctor ===" -ForegroundColor Cyan
& "$repo\target\release\ghost.exe" doctor
if ($LASTEXITCODE -ne 0) { $failed += 'ghost doctor' }

Pop-Location
if ($failed.Count) {
    Write-Host "`nGATE FAILED: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "`nGATE GREEN - safe to package" -ForegroundColor Green
exit 0
```

**Step 2: Run it** (step 3 fails until Task 6 lands — that is expected)

Run: `powershell -NoProfile -File scripts/verify-release.ps1`

**Step 3: Commit**

```bash
git add scripts/verify-release.ps1
git commit -m "build: mandatory pre-release gate (unit + strict live + doctor)"
```

---

## Task 6: `ghost doctor`

The single biggest support-cost reducer for a paid kit, and the fix for "fails on other machines" being invisible until a customer hits it.

**Files:**
- Create: `crates/ghost-cli/src/doctor.rs`
- Modify: `crates/ghost-cli/src/main.rs` (register the subcommand)

**Step 1: Write unit tests for the check logic** (pure functions returning `CheckResult { name, status, detail }`, so they are testable without a desktop).

**Step 2: Implement checks**, each printing `PASS` / `WARN` / `FAIL`:

- Windows build >= 10.0.19041
- Interactive desktop session present (not Session 0)
- UIA `CoCreateInstance` succeeds
- Process is DPI-aware; report the primary monitor's scale factor
- Screen capture returns a non-empty frame
- Vision credentials present — **WARN, not FAIL** (vision is optional; a missing key must degrade, never break)
- `ghost-mcp.exe` resolvable on PATH or at the pinned `~/.local/bin` location

Exit code 0 if no FAIL, 1 otherwise.

**Step 3: Run** `cargo run --release -p ghost-cli -- doctor` and paste the output.

**Step 4: Commit.**

---

## Task 7: DPI-awareness audit

Non-DPI-aware coordinates are the classic silent cross-machine break: everything works at 100% scaling and mis-clicks at 150%.

**Step 1:** Grep for the manifest/API setting awareness:
```bash
grep -rn "SetProcessDpiAwareness\|DPI_AWARENESS\|dpiAware" crates/ ghost*.manifest build.rs 2>/dev/null
```

**Step 2:** If absent, add `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` at startup in all three binaries.

**Step 3:** Verify by setting the display to 150% scaling, then running the text-surface matrix. Coordinates must still land. **This step requires the user to change display scaling — ask before doing it, and restore the original setting afterward.**

**Step 4:** Commit.

---

## Task 8: Per-verb latency instrumentation (measure, do not optimize)

**Do not optimize anything in this task.** Boot is already 20-27ms and capture routing is already correct. The deliverable is *data*.

**Step 1:** Add a timing wrapper in `crates/ghost-mcp/src/main.rs` emitting `{verb, duration_ms, ok}` to stderr when `GHOST_TRACE=1`.

**Step 2:** Run a real session with `GHOST_TRACE=1` for ~20 typical calls; collect the log.

**Step 3:** Report the p50/p95 per verb. Then decide whether Ghost or the agent round-trip dominates.

**Step 4:** Write the conclusion into the design doc — **including "Ghost is not the bottleneck"** if that is what the data says. Commit the instrumentation plus the finding.

---

# TRACK B — DISTRIBUTION

Repo: `~/projects/active/northtek-site` unless stated. **Buy button ships disabled until Track A Task 5 is green.**

## Task 9: Kit packaging script

**Files:**
- Create: `scripts/package-kit.ps1` (ghost repo)
- Create: `dist/quick-start.md`, `dist/mcp-config.json` (ghost repo)

The script must: call `verify-release.ps1` and **abort on failure**; build the three release binaries; assemble `ghost-kit-v<version>-win-x64.zip` containing `ghost.exe`, `ghost-http.exe`, `ghost-mcp.exe`, `quick-start.md`, `mcp-config.json`, `examples/`; emit a SHA256 next to it.

`quick-start.md` must include the **SmartScreen section** — binaries are unsigned, so document the "More info → Run anyway" path plainly and state that the source is on GitHub for anyone who prefers to build it themselves.

Verify by running it and unzipping to a clean directory, then running `ghost.exe doctor` from there.

## Task 10: `ghost.html` landing + purchase page

Dark aurora identity per `~/projects/active/northtek-site/DESIGN.md` — single cyan accent, green reserved for semantic status only, dark-first, elevation via lighter surfaces not shadow stacks. Content: the background-without-focus-steal wedge, honest capability matrix (Windows works; macOS/Linux are scaffolds), the free build-from-source option shown with equal prominence to the $20 kit, and an unsigned-binary note.

Buy button renders `disabled` behind a `KIT_AVAILABLE` flag until Track A is green.

## Task 11: `api/ghost/checkout.js`

Stripe Checkout, `mode: 'payment'` (one-time, **not** subscription), dedicated `STRIPE_PRICE_GHOST_KIT`. Reuse `lib/social-security.js` helpers (`applyCors`, `apiHeaders`, `enforceRateLimit`, `readBody`, `isEmail`) as `api/license/checkout.js` does. `success_url` → `https://northtek.io/ghost-activated?session_id={CHECKOUT_SESSION_ID}`.

## Task 12: `api/ghost/webhook.js` — raw body

**The known-broken pattern must not be copied.** `api/license/webhook.js:68` passes `JSON.stringify(req.body)` to `constructEvent` without disabling the body parser; Stripe verifies against exact raw bytes, so that cannot succeed.

```js
export const config = { api: { bodyParser: false } };

async function readRawBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks);
}
```

On `checkout.session.completed`: generate `NTK-GHOST-xxxx` with `randomBytes` (never `Math.random`), store in KV with `{ downloads: 0, maxDownloads: 10, revoked: false }`, write the `session:<id>` reverse index, email via Resend.

**Test:** verify with the Stripe CLI (`stripe listen --forward-to`) and paste the 200 response. A signature failure here is silent revenue loss.

## Task 13: `api/ghost/download.js` — gated artifact

Vercel Blob, **private**. Validate the license from KV, reject revoked, enforce `downloads < maxDownloads`, increment atomically, return a **5-minute** signed URL. Rate-limit per IP and per key.

Do not place the zip under `/downloads/` — that path is public and would bypass payment entirely.

## Task 14: `ghost-activated.html`

Calls `api/license/lookup`-equivalent for Ghost, shows the license key, a download button, remaining downloads, and the SmartScreen instructions.

## Task 15: README

Rewrite Option A to point at the live page, state binaries are unsigned, and keep build-from-source equally prominent. Ghost repo, so: `Northtek <info@northtek.io>`, no Claude trailer.

## Task 16: Stripe test-mode end-to-end

**REQUIRED SUB-SKILL:** use the `live-flow-tester` skill. Drive the real browser through checkout with a test card, confirm the webhook fires, the email arrives, the download link works, and the download cap enforces. Capture evidence. **Do not switch to live keys until this passes.**

## Task 17: Deploy + smoke

Deploy, then smoke the live URL: `/ghost` returns 200, checkout creates a session, `/ghost-activated` renders, and an unauthenticated `api/ghost/download` request is **rejected**.

**REQUIRED SUB-SKILL:** `SaaS Security Auditor` before enabling live keys — this is a payment surface.

Finally, flip `KIT_AVAILABLE` to enable the Buy button **only after Track A Task 5 reports GATE GREEN.**

---

## Definition of done

- [ ] `scripts/verify-release.ps1` exits 0 with strict live tests
- [ ] `test_notepad_type_text` green; text-surface matrix green across all three families
- [ ] No test in the repo can pass without asserting something
- [ ] `ghost doctor` runs clean on a machine that is not this one
- [ ] Task 8 latency conclusion written down, even if it exonerates Ghost
- [ ] Stripe test-mode purchase → email → download → cap enforcement, all evidenced
- [ ] northtek.io/ghost live; README no longer points at a 404
