# Professional Repo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A stranger lands on github.com/NORTHTEKDevs/ghost and can understand, trust, install and use Ghost in minutes.

**Architecture:** The README becomes a short front door (pitch, demo, install, client configs, safety summary, FAQ) and every deep section moves, unchanged in substance, into focused pages under `docs/`. Internal engineering records stay where they are (moving 580 files would break CHANGELOG links, source comments and the 0.24.4 release evidence) and are labelled internal by a `docs/README.md` index plus a README in each internal folder. One small code fix (tab navigate timeout validation) and one hook fix (commit guard stdin bypass) ride along with tests.

**Tech Stack:** Rust workspace (ghost-mcp), GitHub Actions, Node (hook + link check + demo capture), Python PIL (GIF assembly), gh CLI.

**Spec:** Kristian's request of 2026-11-27 00:34 (session prompt); no separate spec file.

## Global Constraints

- Never touch tags, release assets, or workflow enable/disable (session b3935c owns tag v0.24.4). Release: notes only, via `gh release edit v0.24.4 --notes-file`.
- No release 0.24.5. A code change is pushed to main and held.
- Never stage `.gitignore` or the pre-existing untracked files (`.claude/ .gemini/ .kiro/ AGENTS.md GEMINI.md opencode.json .mcp.json .ignore .github/copilot-instructions.md *.ps1` at root). `git add <path>` only; check `git diff --staged --name-only` before every commit.
- Demo media only from Ghost-owned surfaces: a hidden desktop Ghost creates, `ghost-testbed`, or a Ghost-launched headless browser. Never the user's screens, never the clipboard, never `ghost_stop`/`ghost_reset`, no machine-wide stop.
- Commits: conventional, author Northtek <info@northtek.io>, no Co-Authored-By trailer. Push normally, never force.
- Cargo: stable msvc toolchain on PATH, `CARGO_TARGET_DIR=target-improve`.
- No history rewrite; scrub tracked files only.

## Review Focus

- A visitor on a phone or with images off: README must still read top to bottom; the demo GIF needs alt text and a size under ~1.5 MB.
- Copy-paste configs: every client snippet must name a real binary path pattern for both Windows and Linux, and JSON must parse.
- Doc drift: `docs/tools.md` must fail a test the moment a tool is added, removed or re-described.
- Link rot: every relative link and anchor in README.md and docs/*.md must resolve (checked by script, not by eye).
- Hook fail-open regression: the stdin fix must not start denying commits it cannot see (plain `-m`, `-F <file>`, `--amend --no-edit`).

---

### Task 1: ghost_tab_navigate rejects bad timeout_ms like ghost_tab_open

**Files:**
- Modify: `crates/ghost-mcp/src/ext.rs` (tab_open_wait ~44-59, ghost_tab_navigate ~159-170, tests ~650-667)

**Interfaces:**
- Produces: `fn tab_timeout_ms(p: &Value, tool: &str) -> Result<u64, String>` (default 30000, 1..=600000, else `invalid params: <tool> timeout_ms must be an integer from 1 to 600000 (got <v>)`); `tab_open_wait` calls it.

- [ ] Step 1: test `tab_navigate_timeout_arguments`: `{}` -> 30000; 5000 -> 5000; 600000 -> 600000; 0, -1, 600001, "5000" -> Err starting `invalid params: ghost_tab_navigate`.
- [ ] Step 2: run `cargo test -p ghost-mcp tab_navigate_timeout` -> FAIL (fn missing).
- [ ] Step 3: implement `tab_timeout_ms`, use it in both tools; navigate validates before touching the browser.
- [ ] Step 4: `cargo test -p ghost-mcp` + `cargo clippy --workspace -- -D warnings` -> PASS.
- [ ] Step 5: CHANGELOG `[Unreleased]` entry; commit `fix(mcp): ghost_tab_navigate rejects an invalid timeout_ms`.

### Task 2: commit guard inspects `git commit -F -`

**Files:**
- Backup: `~/.claude/hooks/sd-commit-msg-guard.cjs.bak-<timestamp>`
- Modify: `~/.claude/hooks/sd-commit-msg-guard.cjs` (extractMessage)
- Test: `~/.claude/evals/commit-msg/judge.test.cjs`

- [ ] Step 1: tests: `-F - <<'EOF'` heredoc with "fix stuff" -> deny; same with a good conventional message -> allow; `printf 'wip' | git commit -F -` -> deny; `cat msg.txt | git commit -F -` reads msg.txt; uninspectable stdin (`some-cmd | git commit -F -`) -> deny with "cannot inspect" reason; `-m "feat: x"` unchanged.
- [ ] Step 2: run `node --test ~/.claude/evals/commit-msg/judge.test.cjs` -> new tests FAIL.
- [ ] Step 3: when the -F value is `-`, take the message from a heredoc on the command, else from `echo/printf '<msg>' |` or `cat <file> |`, else return a sentinel that `run` turns into a deny.
- [ ] Step 4: tests PASS; report the diff against the backup.

### Task 3: scrub tracked files

- [ ] Step 1: replace `C:\Users\Krist\projects\active\ghost` (all slash/escape variants) with `<repo>`, then any remaining `Users\Krist` with `Users\<user>`, in `git grep -I -l` hits only.
- [ ] Step 2: `crates/ghost-browser/Cargo.toml` repository -> NORTHTEKDevs; drop the generator comment in CRYSTAL.md.
- [ ] Step 3: verify: `git grep -I -c -E 'Users[\\/]+Krist'` -> no output; secret pattern scan -> no output.
- [ ] Step 4: commit `chore: scrub personal paths from tracked bench logs`.

### Task 4: docs/ for users, internal records labelled

**Files:** Create `docs/README.md`, `docs/install.md`, `docs/configuration.md`, `docs/security-model.md`, `docs/background-mode.md`, `docs/tools.md`, `docs/benchmarks.md`, README in `docs/plans`, `docs/benches`, `docs/audits`, `docs/research`. Create `crates/ghost-mcp/src/tool_reference_tests.rs`.

- [ ] Step 1: test `tool_reference_matches_tools_list` (Windows): `docs/tools.md` == `render_tool_reference(&tools_schema())`; on other OSes, every advertised name appears in the file. Regenerate with `GHOST_WRITE_TOOL_REFERENCE=1`.
- [ ] Step 2: run -> FAIL (no file); generate; run -> PASS.
- [ ] Step 3: move README deep sections into the pages above, substance unchanged.

### Task 5: README front door + demo

- [ ] Step 1: `scripts/demo-capture.mjs`: spawn `ghost-mcp`, `ghost_desktop_create`, launch `ghost-testbed`, type into Field, click Increment, capture each step, close the desktop. Assemble `assets/demo.gif` with PIL (call + capture + `verified` per frame).
- [ ] Step 2: README: pitch, badges (CI, Linux, release, license, MCP registry), demo, features, safety model, quickstart (Claude Code, Claude Desktop, Cursor, VS Code, Codex, generic), platforms, troubleshooting, FAQ, links.
- [ ] Step 3: render with `gh api markdown` + GitHub CSS in a Ghost-launched headless browser; design-director, max 3 cycles.

### Task 6: community files

- [ ] CODE_OF_CONDUCT.md (Contributor Covenant 2.1), `.github/ISSUE_TEMPLATE/feature_request.yml` + `config.yml`, `.github/PULL_REQUEST_TEMPLATE.md`, `.github/dependabot.yml` (cargo + actions, monthly, grouped), SECURITY.md private-advisory link, CONTRIBUTING links, CHANGELOG `[Unreleased]` + heading fix.

### Task 7: gates and ship

- [ ] `scripts/check-links.mjs` over README.md and docs/*.md (files + anchors; external URLs HEAD-checked) -> 0 broken.
- [ ] cargo test + clippy (target-improve) GREEN; `code-vibecode-lint.mjs` 0 HIGH; code-reviewer until no HIGH/MEDIUM.
- [ ] Commit per task, push, CI green on the pushed head.
- [ ] `gh repo edit` description/topics/homepage; polish v0.24.4 notes only.
