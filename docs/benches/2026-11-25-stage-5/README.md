# Stage 5 - review follow-ups

## (a) verify: a Ghost-launched window holding the foreground at the end fails

The interference audit records only synthetic foreground changes (no hardware
input in the previous 1.5 s). A window Ghost raised while the human was typing
is classed as the human's change; `audit.rs` keeps only a count of those
(`human_foreground_changes`), not where the foreground went, so no incident can
show it. `foreground_verdict` (`crates/ghost-cli/src/verify.rs`) now fails
whenever the final foreground window belongs to a process the run launched, with
or without an audit.

Not covered: a Ghost-raised window that the human then left before the end of
the run. That needs the audit to record human-classed changes to Ghost-driven
windows; it is a convergence-round candidate, not part of this change.

| Criterion | Target | Measured |
|---|---|---|
| Unit: human typing (no incident) + Ghost window in front at the end | FAIL | FAIL; red before the rule, green after |
| Unit: the same with the audit off | FAIL | FAIL |
| Earlier 8 verify unit cases | unchanged | 10/10 verify tests green |
| `ghost verify` x3 against target-improve | 3/3 pass | 3/3 pass (`a-verify-run*.txt`) |
| `cargo test --workspace` | 0 failed | 577 / 0 / 27, both runs |
| clippy, vibecode lint on changed files | 0 / 0 HIGH | 0 / 0 findings |

## (b) act paths refuse only a definitely disabled element

`UiaElement::is_enabled` (`crates/ghost-core/src/uia/element.rs`) mapped a UIA
error on `CurrentIsEnabled` to `false`, and all three act paths refused on
`!is_enabled()`, so an element whose provider fails that one property read was
refused as "disabled" and never driven. Now:

- `UiaElement::is_enabled_checked() -> Option<bool>` (`None` = no answer);
  `is_enabled()` keeps mapping an error to `false`, so describe/snapshot output
  is unchanged. Linux (AT-SPI state set, always definite) returns `Some`; the
  macOS stub returns `None`, so a macOS act reports the real `Unsupported`
  instead of a false "element is disabled".
- `ghost-session`: `refuse_as_disabled(Option<bool>)` refuses only `Some(false)`.
  Every action gate uses it: `GhostElement` click / type_text /
  click_background (element.rs), `act_on_element` and `act_background`
  (session.rs), and the hidden path's `Located.disabled` (hidden.rs).

A provider that errors on IsEnabled cannot be produced on demand here, so the
unit tests cover the decision and the probes show the definite cases unchanged.
The Linux and macOS edits could not be cross-checked on this machine: `ring`'s
build script needs a C cross-compiler (`b-check-*.txt`); they are one additive
method each.

| Criterion | Target | Measured |
|---|---|---|
| Unit: Some(false) refused, Some(true) and None not | as stated | 3/3; red on a stub, green after |
| Disabled `Locked`, user / hidden: refused, same error | 20/20, `Element not interactable: Locked - element is disabled` (-32002) | 20/20 / 20/20, same message and code |
| Increment, user / hidden: correct and verified | 20/20 | 20/20 / 20/20 |
| Label, user / hidden: verified=false | 20/20 | 20/20 / 20/20 |
| `cargo test --workspace` | 0 failed | 580 / 0 / 27, both runs |
| clippy, vibecode lint on changed files | 0 / 0 HIGH | 0 / 0 findings |
