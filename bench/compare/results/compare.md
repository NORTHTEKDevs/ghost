# Cross-tool comparison (measured)

Only tools with an implemented adapter are RUN here. Columns without an adapter show `-` (not measured) — this harness never asserts competitor numbers it didn't run. To add a tool, see [README.md](../README.md).

| Task | Ghost | Playwright-MCP | Computer Use | cua-driver |
| --- | --- | --- | --- | --- |
| Launch Calculator, compute 6 x 7, read the display = 42 | PASS | - | - | - |
| Type text into a native field and read it back exactly | PASS | - | - | - |
| Operate a native Win32 app with no API/CDP (Character Map) | PASS | - | - | - |
| Act inside an app WITHOUT taking foreground or moving the cursor | PASS | - | - | - |
| Report per-action whether the effect actually happened | PASS | - | - | - |
| Read the UI as structured data (elements/text), not just a screenshot | PASS | - | - | - |

`-` = no adapter run (not measured). `N/A` = tool can't attempt the task (e.g. a browser-only tool on a native-app task).
