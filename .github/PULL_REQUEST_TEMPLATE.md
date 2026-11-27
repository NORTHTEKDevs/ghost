## What and why

<!-- What changes for someone using Ghost, and why. Link the issue if there is one. -->

## How it was tested

<!-- The test that fails without this change, and the commands you ran. Live desktop
behaviour needs a live test in the ignored suite; say which platform you ran it on. -->

## Checklist

- [ ] A test fails without this change (or it is docs only)
- [ ] `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` pass
- [ ] No silent fallback: a verb that cannot do what was asked returns an error naming why
- [ ] Nothing takes the user's foreground under the default `background` policy
- [ ] A CHANGELOG entry under `[Unreleased]`
- [ ] If a tool's schema changed: `docs/tools.md` regenerated (`GHOST_WRITE_TOOL_REFERENCE=1 cargo test -p ghost-mcp tool_reference`)
