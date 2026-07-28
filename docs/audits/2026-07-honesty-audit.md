# Ghost honesty audit — 2026-07-27

**Purpose.** Walk every user-facing surface (README, crate descriptions, MCP
registry entry, kit copy, docs) and flag any wording that overstates what Ghost
actually does today. For each flag, give the exact fix. This is the "strict
honesty" pass: **Ghost is Windows today, macOS/Linux are scaffolds that don't yet
run** — every surface must say that or say nothing about non-Windows.

**Baseline of truth (from the code):**

- `crates/ghost-platform/src/lib.rs::capabilities_for()` returns
  `functional: true` for Windows and `functional: false` for macOS/Linux with an
  explicit "scaffold — native backend … not yet implemented/verified" status
  string.
- `crates/ghost-platform/src/{macos.rs,linux.rs}` contain inert `MacBackend` /
  `LinuxBackend` structs. No AXUIElement, no AT-SPI, no CGEvent, no XTest, no
  ScreenCaptureKit code.
- Every Windows-flavored crate (`ghost-core`, `ghost-session`, `ghost-cli`,
  `ghost-http`, `ghost-mcp`) unconditionally depends on the `windows` crate.
  They will not build off Windows.
- CI (`.github/workflows/ci.yml`) runs `windows-latest` only. There is no macOS
  or Linux job. No target currently proves "compiles on Darwin/Linux."

Everything below is measured against that baseline.

---

## Severity legend

- **S1 — Overclaim** — reader will reasonably believe Ghost works on a platform
  where it does not. Must be fixed before v0.17.
- **S2 — Ambiguous / inconsistent** — different surfaces contradict each other,
  or a single surface says both "three-version" and "Windows only." Fix in the
  same release.
- **S3 — Stale metadata** — versions, tool counts, crate descriptions drifting
  out of sync. Fix at the next release cut.

---

## 1. `README.md` — the front door

### S1-a. "Platforms" block (README lines ~50–61)

Current:

> Ghost targets three OSes through one shared contract (`crates/ghost-platform`):
> - **Windows** — full and verified. …
> - **macOS / Linux** — architecture in place, native backends in progress (not yet functional). …

**Verdict.** Honest in spirit, but "targets three OSes" is stronger than a first-
time reader will parse. The word "targets" implies build-and-run today; combined
with the badge-style bullet layout it reads like a support matrix, not a roadmap.
The tail sentence ("The macOS … engines are scaffolded") is where the truth
lands, but by then a scanning reader has already banked "three platforms."

**Fix (replace the block):**

```md
### Platforms

Today Ghost runs on **Windows 10/11 only.** macOS and Linux are on the roadmap
and have a shared cross-platform contract in place (`crates/ghost-platform`),
but the native backends are not written yet — those binaries will not build or
run until they are. See [`docs/cross-platform.md`](docs/cross-platform.md) for
the capability matrix and the plan.

Ghost's background-without-focus-steal wedge relies on Windows posted window
messages; that specific capability has no exact macOS/Linux equivalent and will
be re-measured once the native backends land.
```

Rationale: leads with the load-bearing sentence ("Windows 10/11 only"), keeps
the cross-platform ambition but demotes it to roadmap language, still points to
the honest capability doc.

### S1-b. Opening tagline (README line 3)

Current:

> **The computer-use layer for AI agents on Windows.**

**Verdict.** This one is fine — it already says "on Windows." Keep as-is.

### S1-c. "Ship it three ways" (README lines ~40–43)

Current:

> - **`ghost` CLI** …
> - **`ghost-http` server** …
> - **`ghost-mcp` server** — Model Context Protocol server for Claude, Cursor, and any MCP client (**37 tools**)

**Verdict.** Two problems.

1. Tool count mismatch. `ghost-mcp/src/main.rs` exposes 63 unique
   `ghost_*` tool names (see `grep -oE '"ghost_[a-z_]+"' | sort -u | wc -l`),
   while `Cargo.toml`'s crate description says "20 lean Ghost desktop + shell
   automation tools" and the README says "37 tools." These three numbers must
   agree.
2. The count of 20 is intentional ("20 lean verbs advertised (legacy names stay
   dispatchable)" — CHANGELOG 0.16.0, README lower "Quick Start — Claude
   Desktop / MCP" section). That sentence is the correct one; the "37 tools"
   line contradicts it in the same document.

**Fix.** Pick **one** number — the advertised verb count — and use it
consistently. Recommended:

- README top line: `Model Context Protocol server for Claude, Cursor, and any MCP client (20 verbs; legacy tool names stay dispatchable).`
- `crates/ghost-mcp/Cargo.toml::description`: match. Currently correct at 20.
- The lower "Quick Start — Claude Desktop / MCP" block already says "20 lean
  verbs advertised" — keep.

### S2-a. Install section is Windows-only but doesn't say so

Current (lines ~64–90): "Ready-to-run kit ($20)" and `cargo build --release
--bin ghost --bin ghost-http --bin ghost-mcp` are described with no OS
qualifier, then "Requirements: Windows 10 build 19041+" appears as a
parenthetical at the end.

**Fix.** Move the Windows requirement to the section header, not the tail:

```md
## Install (Windows 10/11)

Ghost binaries only build and run on Windows today. macOS/Linux support is
tracked in [`docs/cross-platform.md`](docs/cross-platform.md).
```

Then the two install options as they are. Reader can't miss the requirement.

### S2-b. "Vision grounding" and "Background mode" sections implicitly assume Windows

These sections describe Win32 posted window messages, `WM_SETTEXT`, `BM_CLICK`,
`PrintWindow`, `UIA Invoke/SetValue`, etc. On a page whose "Platforms" block
mentions macOS/Linux the reader may assume the same primitives will exist there.

**Fix.** Add one sentence at the top of each Win32-specific section:

- Background mode: `Windows-only today (posted messages are a Win32 primitive; the macOS/Linux equivalents will be measured when those backends land).`
- Set-of-Marks canvas fallback: no change (already OS-agnostic via `ghost-ground`).

### S3-a. Architecture ASCII diagram (README lines ~305–315)

Current:

```
ghost-cli     ghost-http     ghost-mcp     Rust SDK
    \            |              /             |
      +-----> ghost-session  <----------------+   ← safe Rust API
                   |
              ghost-core                         ← Win32 FFI: UIA, SendInput, DXGI
                   |
              Windows OS
```

**Verdict.** Truthful (says "Windows OS" at the bottom) but omits
`ghost-platform`. Since we're leaning on that crate as the honest cross-
platform contract, it should appear in the diagram.

**Fix.**

```
ghost-cli     ghost-http     ghost-mcp     Rust SDK
    \            |              /             |
      +-----> ghost-session  <----------------+   ← safe Rust API (today: Windows-only)
                   |
              ghost-core                         ← Win32 FFI: UIA, SendInput, DXGI
                   |
              ghost-platform                     ← cross-OS contract; Win backend today, mac/linux scaffolded
                   |
              Windows OS
```

---

## 2. `docs/cross-platform.md`

Read against the code, this doc is the **most honest surface Ghost has today**.
It explicitly says "not functional," "must be built and verified on-device,"
"only claimed once tested," and names Windows as the flagship. **Keep as-is.**
The rest of the audit is basically "make every other surface as honest as this
one."

Minor S3: table row for Windows says "verified" without a link to how it was
verified — link to `bench/results/latest.md` for the reader who wants to check.

---

## 3. `docs/comparison.md`

### S1-d. cua-driver row

Current:

> **cua-driver (Hermes)** — you're inside the Hermes agent and want cross-platform
> (mac/Windows/Linux) background control. Similar background philosophy to Ghost;
> Ghost adds per-action verification and deeper Windows UI Automation.

**Verdict.** Fine as competitor context, but the comparison table above it lists
Ghost with ✅ across several rows without noting that those ✅s are
Windows-only. A reader comparing "Ghost vs cua-driver" for cross-platform work
will misread the table.

**Fix.** Add a Platform row at the top of the table:

| Capability | **Ghost** | Playwright-MCP | Anthropic Computer Use | cua-driver (Hermes) | pywinauto / WinAppDriver |
| --- | --- | --- | --- | --- | --- |
| Platform coverage | ⚠️ Windows only today; mac/linux scaffolded | any (via browser) | mac/win/linux VM | mac/win/linux | Windows |

Every ✅ in the rest of the table then reads "on Windows," and the "When to
choose each" text already implicitly acknowledges Windows-only for Ghost.

The "honest caveat" paragraph at the bottom is good; don't touch it.

---

## 4. `server.json` (MCP registry submission)

Current:

```json
"description": "Computer-use MCP server for Windows. Drives any native app …",
"version": "0.15.1",
```

**Verdict on wording.** Honest — already says "for Windows." **Keep the
description.**

### S3-b. Version drift

- `server.json.version` = `0.15.1`
- `crates/ghost-mcp/Cargo.toml.version` = `0.16.0`
- Latest CHANGELOG entry = `0.16.0 — Shell control`

**Fix.** Bump `server.json` to `0.16.0` (both top-level `version` and
`packages[0].version`). Every release-tagged surface should agree with the crate
version.

### S3-c. runtimeHint mentions only Windows install path

Current: `"Prebuilt ghost-mcp.exe (Windows 10/11), or `cargo build --release -p ghost-mcp`."`

**Verdict.** Correct today. Keep.

---

## 5. `kit/mcp-config.json` and `kit/install.ps1`

Both are Windows-only artifacts (`.exe`, PowerShell). No claim about mac/linux
appears in either. **Keep as-is.** When macOS backends ship, add a sibling
`kit/install.sh` (mac + linux) and a `mcp-config.mac.json` — do **not** publish
those until backends are verified on-device.

---

## 6. Crate descriptions on crates.io

These show up on the crates.io landing page and MUST agree with the README.

| Crate | Current description | Verdict |
| --- | --- | --- |
| `ghost-cli` | "Command-line interface for Ghost Windows desktop automation" | ✅ honest |
| `ghost-core` | "Low-level Win32/UIA/SendInput/DXGI primitives for the Ghost desktop automation framework" | ✅ honest |
| `ghost-http` | "HTTP REST server for Ghost Windows desktop automation" | ✅ honest |
| `ghost-mcp` | "MCP server binary exposing **20** Ghost desktop + shell automation tools over stdio JSON-RPC" | ✅ honest; keep 20 |
| `ghost-session` | "Safe async session API for Ghost Windows desktop automation" | ✅ honest |
| `ghost-ground` | "Hybrid grounding cascade for Ghost — coordinate contract, types, parser, engine, and YOLO tier." | ✅ OS-agnostic wording is fine |
| `ghost-platform` | "Cross-platform contract for Ghost: the capability model, shared types, and per-OS backend selection (Windows full; macOS/Linux scaffolded)." | ✅ this is exactly the right honest wording |

**No changes required.** These are the model. They are strictly more honest
than the README.

`keywords = ["…", "windows", "mcp", "agent", "claude"]` on `ghost-mcp` — keep,
but when macOS lands, add `"macos"` there and to `categories`.

---

## 7. `CRYSTAL.md`

Internal notes file, no external claim. No change needed.

---

## 8. Landing-page copy (northtek.io/ghost)

Not in the repo, but the README links to `https://northtek.io/ghost` for the
paid kit. **Recommend the same wording pass on that page** — it's the highest-
stakes surface (money changes hands). If the current landing copy uses the
"three ways / three platforms" framing, downgrade the platforms line to
"Windows 10/11 today; macOS/Linux in progress" before shipping the v0.17
release.

Ask: does the paid kit page currently mention macOS/Linux? If yes, that's an S1
overclaim on a paid page — the biggest of the whole audit and the one to fix
first, since it plausibly touches consumer-protection concerns.

---

## Summary — must-fix before next release

1. README "Platforms" block → **strict Windows-only lede** with roadmap tail. (S1-a)
2. Move "Windows 10/11" to `## Install` header, not tail parenthetical. (S2-a)
3. Fix tool count: README "37 tools" → "20 verbs" to match `ghost-mcp` crate
   description and CHANGELOG 0.16.0. (S1-c)
4. `docs/comparison.md` table gets a "Platform coverage" row that says
   "Windows only today; mac/linux scaffolded." (S1-d)
5. `server.json` version bump 0.15.1 → 0.16.0. (S3-b)
6. Verify the northtek.io landing copy does not overclaim cross-platform. (S1
   candidate — needs eyes on the live page)
7. Add `ghost-platform` node to the README architecture diagram. (S3-a)

---

## What this audit does **not** touch

- Any technical claim about Windows (background dispatch, per-action verify,
  Set-of-Marks accuracy, bench 14/14). Those are verifiable and I have no
  reason to doubt them; a code-level audit of the bench harness is a separate
  pass if you want it.
- The cross-platform *plan* itself. That's Pass 2 — the implementation-plan
  doc — coming next.
