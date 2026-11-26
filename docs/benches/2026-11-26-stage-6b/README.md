# Stage 6b - C10, C9, C11

Probe-safety rule: every probe sends input only to testbed windows it launched;
edit contents are read with WM_GETTEXT (a read, no input) unless a row says
otherwise.

## C10 - a background ghost_key combo is never posted as its bare key

**Defect.** Under the default background policy (`window` given, no
`background` flag), `handle_ghost_key` did `let (_, key_only) =
parse_key_combo(keys)?` and posted `key_only`: every combo's modifiers were
dropped. Measured (`c10-before`): "Ctrl+A", "Ctrl+S", "Ctrl+Shift+K" and "Alt+X"
each typed their bare letter into the focused edit, 10/10 each, 40/40 - so
"Ctrl+S" put an "S" into the document. The comment above the code said combos
were refused.

**Change** (`crates/ghost-mcp/src/main.rs`). A pure `background_key_plan(mods,
key)` decides: no modifiers -> post the key; Ctrl+C/X/V/A/Z -> the semantic edit
command (WM_COPY/CUT/PASTE/UNDO, EM_SETSEL); any other combo -> refuse.
`key_in_background` executes the plan and is now the only background key path,
used by both `background=true` and the background policy, so they cannot
disagree again. The refusal says the combo was not sent, why, which shortcuts
work in the background, and that other combos need the operator-unlocked
foreground policy.

| Criterion | Target | Before (`c10-before`) | After run 1 / run 2 |
|---|---|---|---|
| Combos that typed their bare key (4 combos x 10) | 0/40 | **40/40** | 0/40 / 0/40 |
| Ctrl+A delivered as the select_all edit command | 10/10 | 0/10 (typed "A") | 10/10 / 10/10 |
| Ctrl+S, Ctrl+Shift+K, Alt+X refused with the reason | 30/30 | 0/30 | 30/30 / 30/30 |
| A single key still delivered (`c10-key-regression`, decoy case) | target 20/20, decoy 0 | - | 20/20, 0 |
| Unit: single key -> post; Ctrl+A/C/V -> edit command; 7 other combos -> refuse | as stated | 2 of 3 red on a stub | 3/3 green |
| `cargo test --workspace` | 0 failed | 585 / 0 / 27 | 588 / 0 / 27, both runs |
| clippy, vibecode lint | 0 / 0 HIGH | - | 0 / 0 findings |
