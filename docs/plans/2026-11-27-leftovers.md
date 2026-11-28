# Ghost leftovers after 0.24.5 - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the items 0.24.5 left open (two dependency ports, one timeout rule, the demo script cleanup, the README bar) and the listing work on GitHub, Glama and Smithery.

**Architecture:** Each code item is its own branch and PR, squash-merged once CI is green. A 0.24.6 release follows only if shipped behavior changed. The web items run in Kristian's own signed-in browser through Comet MCP and Ghost MCP.

**Tech Stack:** Rust workspace (windows-rs, tokio-tungstenite), Node dev scripts, GitHub Actions, gh CLI.

**Spec:** Kristian's request of 2026-11-27 14:42 (items 1-8), plus the 0.24.5 review's open LOW findings.

## Global Constraints

- cargo always `-j 2` with `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=target-improve`; no new target dirs (C: has ~24 GB free).
- One branch + PR per code item; CI green; squash-merge with a conventional subject. No force-push, no Co-Authored-By.
- Never stage `.gitignore` or the untracked files. Never kill processes this session did not start. Leave session 7da82d alone.
- Testing touches only Ghost-owned windows. The web items (6-8) may use Kristian's browser.
- Web limits: stop an item at a 2FA code, passkey or captcha; report OAuth scopes; no paid plans or trials; no security-setting changes; no emails, support messages or posts. A Smithery support request is drafted below, not sent.
- Gates before release: code-reviewer no HIGH/MEDIUM; `code-vibecode-lint` 0 HIGH; workspace tests + clippy `-D warnings` on Windows; Linux through CI.

## Review Focus

1. A wait tool given `timeout_ms: 0`. Expected: invalid params naming the range, the same on every wait tool. Pinned in Task 3.
2. A wait tool given `timeout_ms` above 600000 (it used to be capped silently). Expected: invalid params, not a silent cap. Pinned in Task 3.
3. CDP text and binary frames after the tungstenite port: non-ASCII text, and frames over 64 KB (screenshots). Expected: byte-identical payloads. Pinned in Task 2 by the existing CDP tests plus a non-ASCII round trip.
4. Hidden-desktop launch and posted input after the windows-crate port: handle types and `BOOL`/`Result` signatures changed. Expected: behavior unchanged. Pinned in Task 1 by the full workspace suite on Windows CI plus the hidden-desktop tests.
5. demo-capture.mjs when the launch reports a non-hidden surface. Expected: the testbed it started is closed and the script exits non-zero. Pinned in Task 4.

---

### Task 1: Port windows 0.58 -> 0.62 (Dependabot #14)

**Files:** Cargo.toml / crate manifests that name `windows`; compile errors in `crates/ghost-core/src/{desktop.rs,input/mouse.rs,input/postmessage.rs,uia/tree.rs,ocr.rs}`, `crates/ghost-browser/src/launch.rs`, and wherever the compiler points next.

- [ ] Branch `deps/windows-0.62` from main; bump `windows` (and `windows-core` if pinned) to 0.62.
- [ ] `cargo check -j 2 --workspace --all-targets` and fix each error mechanically: new handle newtypes, `BOOL` -> `Result`/`bool` changes, moved items. Behavior must stay the same.
- [ ] `cargo test -j 2 --workspace` and `cargo clippy -j 2 --workspace --all-targets -- -D warnings` green on Windows.
- [ ] Commit `deps: port to windows 0.62`, open the PR, CI green, squash-merge; close #14 with a note pointing at the PR.

### Task 2: Port tokio-tungstenite 0.24 -> 0.30 (Dependabot #15)

**Files:** `crates/ghost-browser/Cargo.toml`, `crates/ghost-browser/src/cdp.rs` (lines ~154, 195, 222, 256).

- [ ] Branch `deps/tungstenite-0.30`. Add the test `cdp::tests::non_ascii_text_round_trips` against the existing local echo/fake server pattern in cdp.rs: send `{"x":"é漢🙂"}`, assert the received text is equal.
- [ ] Bump to 0.30; adapt `Message::Text(Utf8Bytes)` / `Message::Binary(Bytes)` construction and matching (`.into()`, `.as_str()`).
- [ ] ghost-browser tests (with `GHOST_REQUIRE_BROWSER=1`) + clippy green; commit `deps: port to tokio-tungstenite 0.30`; PR; CI; squash-merge; close #15 with a note.

### Task 2b: Re-review #16 (base64 0.23)

- [ ] Rebase it (`@dependabot rebase`). Merge only if CI is green and the lockfile no longer carries two base64 versions (i.e. reqwest/hyper-util/ureq/tungstenite moved to 0.23); otherwise leave it open with the existing note.

### Task 3: One timeout rule for every wait tool

**Rule (documented in docs/tools.md and the schemas):** `timeout_ms` is an integer from 1 to 600000; anything else is invalid params naming the tool and the range. Omitted = the tool's default.

**Files:** `crates/ghost-mcp/src/main.rs` (`validate_args` around line 1790, `MAX_WAIT_TIMEOUT_MS`, wait tool schemas), `crates/ghost-mcp/src/arg_validation_tests.rs` (`mod wait_timeout`), `docs/tools.md` (regenerated), `CHANGELOG.md`.

- [ ] Branch `fix/wait-timeout-rule`. Replace `a_huge_timeout_is_capped_on_every_wait` with `out_of_range_timeouts_are_invalid_params_on_every_wait`: for each name in `WAITS` and each of `0`, `600_001`, `1u64 << 62`, `validate_args(name, …)` is `Err` starting `invalid params: {name} timeout_ms must be an integer from 1 to 600000`. Add `in_range_timeouts_pass_untouched` for `1` and `600_000`.
- [ ] Run; expect FAIL (today 0 passes and large values are rewritten).
- [ ] Implement in `validate_args`: reject instead of rewrite, with the same message format as `ext::timeout_ms_arg`. Add `"minimum": 1, "maximum": 600000` to each wait tool's `timeout_ms` schema.
- [ ] Tests green; `GHOST_WRITE_TOOL_REFERENCE=1 cargo test -p ghost-mcp tool_reference` then re-run without it; CHANGELOG `Changed` entry (stricter: 0 and >600000 now rejected); PR; CI; squash-merge.

### Task 4: demo-capture.mjs closes the testbed when the launch is not hidden

**Files:** `scripts/demo-capture.mjs`, new `scripts/demo-capture.test.mjs` (node:test).

- [ ] Branch `fix/demo-capture-cleanup`. Factor `async function launchHidden(call, exe, title)` -> `{ launched: true }` that, when the launch answer's `surface !== 'hidden'`, closes the window it just launched through the anchor (`ghost_act Quit`, falling back to `ghost_window op=close` on the returned anchor) and then throws.
- [ ] Test (fake `call`): a launch answering `{surface:'user'}` produces a close call and a thrown error; `{surface:'hidden'}` produces no close. Run it first against the current logic -> FAIL.
- [ ] Implement; `node --test scripts/demo-capture.test.mjs` green; PR; squash-merge.

### Task 5: README to >= 8.5 (design-director, max 3 cycles)

- [ ] Apply the run-2 consensus fixes (history_ghost-readme.md): 2-column claims table with the link inside the cell, lead with the success ratio, bold Quick start link, GIF wrapped in a link to full size, remove the repeated PATH/update text, "How an agent uses it" above Quick start.
- [ ] Render + 3-lens panel per cycle; Guard 1 reverts a regression. Commit on a branch, PR, merge only the final passing or best non-regressing version.

### Task 6: Release 0.24.6 (only if Tasks 1-4 changed shipped behavior)

- [ ] Gates (code-reviewer, lint, tests, clippy, CI), bump Cargo.toml/Cargo.lock/server.json/plugin.json/lhm.plugin.json/CHANGELOG, `chore(release): 0.24.6`, annotated tag, Release + Registry green, LobeHub `plugin update`, install published binaries (SHA vs sidecar) to ~/.local/bin and target\release, production-certifier.

### Task 7: GitHub social preview (web)

- [ ] In Kristian's browser: github.com/NORTHTEKDevs/ghost/settings -> Social preview -> upload `assets/social-preview.png`. Verify with `gh api graphql` (`openGraphImageUrl`, `usesCustomOpenGraphImage: true`).

### Task 8: Glama claim + Build & Release (web)

- [ ] Sign in (GitHub preferred), claim https://glama.ai/mcp/servers/NORTHTEKDevs/ghost, Admin > Dockerfile with the saved spec from docs/publishing-mcp.md, Build & Release. Record result and any OAuth scopes. Stop at 2FA/captcha.

### Task 9: Smithery listing (web)

- [ ] Sign in (GitHub/Google), find why the Ghost page 404s, fix or relist through the UI. If it needs support, draft the message under "Smithery support draft" below and stop.

## Smithery support draft

Findings (2026-11-27): smithery.ai shut down accounts, API keys, the CLI and
its proxy on 2026-10-31 and is now a curated, read-only directory run by
Arcade.dev. There is no sign-in to use, which is why `smithery mcp publish`
has answered 403 "Updating servers has been removed" since 2026-11-26. The
registry API still serves `info-j5od/ghost` (stdio bundle, 0.23.4), but the web
page for it returns 404. The notice names one contact: ani [at] smithery [dot]
ai. NOT SENT - Kristian sends it if he wants the listing kept.

> Subject: Ghost (info-j5od/ghost) - listing page 404s, update path?
>
> Hi Ani,
>
> We publish Ghost, an open-source MCP server for desktop automation on
> Windows and Linux (MIT, https://github.com/NORTHTEKDevs/ghost). It was
> published to Smithery as `info-j5od/ghost` as a stdio MCP bundle. Since the
> October 31 change, its page at https://smithery.ai/servers/info-j5od/ghost
> returns 404, while registry.smithery.ai still returns the server at version
> 0.23.4, and `smithery mcp publish` answers 403 "Updating servers has been
> removed".
>
> Ghost runs locally over stdio (no hosted URL); current release is 0.24.5,
> listed in the official MCP registry as io.github.NORTHTEKDevs/ghost with
> .mcpb bundles for Windows and Linux. Could you either restore the page with
> the current version, or tell us how local stdio servers are listed in the
> curated directory now? If local servers are out of scope, please remove the
> stale 0.23.4 entry instead.
>
> Thanks,
> Northtek (github.com/NORTHTEKDevs)
