# Ghost vs the alternatives

An honest map of where Ghost fits among computer-use / automation tools. Ghost is
not "best at everything" — it's the best fit for **an AI agent driving native
Windows apps, especially ones with no API, and doing it without hijacking the
screen.** For web scraping or cross-OS VM agents, other tools are the right call.

## At a glance

| Capability | **Ghost** | Playwright-MCP | Anthropic Computer Use | cua-driver (Hermes) | pywinauto / WinAppDriver |
| --- | --- | --- | --- | --- | --- |
| Native Windows apps (Win32/WPF/UWP) | ✅ deep (UI Automation) | ❌ browser only | ⚠️ via screenshots | ✅ | ✅ |
| Web / browser | ⚠️ drives it like a user (no DOM) | ✅ deep (CDP/DOM) | ⚠️ screenshots | ⚠️ | ❌ |
| **Background (no focus/cursor steal)** | ✅ posts window messages | ❌ | ❌ (drives the VM screen) | ✅ | ❌ |
| **Per-action verification** | ✅ `verified`/`focus_confirmed` | partial (waits) | ❌ (model must re-look) | ❌ | ❌ |
| Works on apps with **no API** | ✅ | ❌ | ✅ (visual) | ✅ | ✅ |
| MCP-native (any agent mounts it) | ✅ | ✅ | n/a (Anthropic-native) | ⚠️ via its harness | ❌ |
| Model-agnostic vision | ✅ any OpenAI-compat or Anthropic | n/a | ❌ Claude only | ✅ | n/a |
| Runs on the real desktop (not a VM) | ✅ | ✅ | ❌ needs a VM | ✅ | ✅ |
| Built for autonomous agents | ✅ | ⚠️ | ✅ | ✅ | ❌ (libraries) |

✅ strong · ⚠️ partial/possible · ❌ not really

## When to choose each

- **Ghost** — an agent (or script) needs to operate real Windows applications:
  legacy line-of-business software, vendor portals, desktop apps with no
  integration. Especially when it should run *in the background* while a person
  keeps using the machine, and when you need to *know* each action worked.
- **Playwright / Playwright-MCP** — the target is a website and you want DOM-level
  control, network interception, and headless browsers. Ghost drives a browser
  only as a user would (no DOM); Playwright is the better web tool.
- **Anthropic Computer Use** — you want a Claude-native agent reasoning over
  screenshots inside a sandboxed VM, across OSes. Strong general vision reasoning;
  it operates the VM's foreground screen (not background) and re-looks to confirm.
- **cua-driver (Hermes)** — you're inside the Hermes agent and want cross-platform
  (mac/Windows/Linux) background control. Similar background philosophy to Ghost;
  Ghost adds per-action verification and deeper Windows UI Automation.
- **pywinauto / WinAppDriver / AutoHotkey** — established, free Windows automation
  libraries for scripts and QA. No agent integration, no verification loop, no
  background dispatch — you build those yourself.

## The honest caveat

The table above is **architectural**, not a measured head-to-head benchmark. Ghost
publishes its own reproducible numbers ([`bench/`](../bench)) — task success and a
reliability soak — but a fair cross-tool benchmark requires standing up each
competitor on a shared task set, which is a separate effort. Claims like "fastest"
or "most reliable" should be earned with that benchmark, not asserted. What *is*
verifiable today: the background + verification behavior, which
[`examples/background_agent_demo.py`](../examples/background_agent_demo.py)
demonstrates end-to-end on any Windows 10/11 machine.
