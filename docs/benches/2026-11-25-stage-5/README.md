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
