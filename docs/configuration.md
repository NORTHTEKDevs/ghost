# Configuration

Ghost needs no configuration and no API key. Everything below is optional and
set as environment variables on the server process: in the `env` block of your
MCP client's config, or as checkboxes when you install the MCP Bundle. The
server logs where it stands on the two consent settings at start.

## The two consent settings

| Variable | Default | What it means |
| --- | --- | --- |
| `GHOST_FOCUS_LOCK` | on | The agent can never raise the focus policy, so it drives windows in the background and cannot take your cursor or keyboard. Set `off` only if you want an agent to be able to use your real input. |
| `GHOST_SHELL` | on | `ghost_shell` runs programs with your account's rights (builds, git, scripts). It is full access to the machine. Set `off` to refuse every shell call and filter launches; see the [security model](security-model.md#shell-access-is-on-by-default-and-you-can-turn-it-off). |

Unlocking the focus policy, for an agent that should drive your real mouse:

```json
{
  "mcpServers": {
    "ghost": {
      "command": "ghost-mcp",
      "env": { "GHOST_FOCUS_LOCK": "off", "GHOST_FOCUS_POLICY": "prefer_background" }
    }
  }
}
```

`GHOST_FOCUS_POLICY` picks the starting policy once unlocked: `background`,
`prefer_background` or `foreground`. `ghost_focus_policy` reports the current
policy and whether it is locked.

## Other variables

| Variable | Effect |
| --- | --- |
| `GHOST_LAUNCH_ALLOW` | With `GHOST_SHELL=off`, only these programs may be launched (comma-separated basenames or full paths). |
| `GHOST_HTTP_ALLOW_PRIVATE=1` | Lets `ghost_http_get`/`ghost_http_post` reach loopback, private and link-local addresses. |
| `GHOST_BROWSER_PATH` | The Chromium-family browser `ghost_browser_launch` starts, instead of the one Ghost finds. |
| `GHOST_TOOL_DEADLINE_MS` | Per-call deadline, default 180000 ms. A call's own `timeout_ms` raises it. |
| `GHOST_CDP_ROUTE=off` | Do not route anchored verbs through a browser's DevTools port when its window has one. |
| `GHOST_SHELL_WARM=off` | Do not keep a pre-started PowerShell for fast `ghost_shell op=run` calls. |

## Optional vision tier

An agent driving Ghost over MCP does its own looking: `ghost_see` returns every
element with its name, role and position, `ghost_see mode=text` returns the
readable text, and `ghost_screenshot` returns pixels. Ghost never needs a
vision key for that.

The built-in vision tier is for callers with no model of their own (the CLI,
the HTTP API, an intent file) and for `ghost_find description="the blue Submit
button"`. It works with any tool-capable vision model behind an
OpenAI-compatible endpoint (OpenAI, Gemini, Groq, NVIDIA, a local vLLM, Ollama
or LM Studio) or Anthropic.

| Variable | Value |
| --- | --- |
| `GHOST_VISION_BASE_URL` | The endpoint. A keyless local server needs only this. |
| `GHOST_VISION_MODEL` | The model name. |
| `GHOST_VISION_API_KEY` | The key, if the endpoint needs one. |
| `GHOST_VISION_PROVIDER=anthropic` | For Anthropic's API. |

Ambient `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` and `NVIDIA_API_KEY` are ignored,
so a screenshot is never sent to a provider because of an unrelated key.
Without the tier, description targets return an error naming the alternative;
every other tool is unaffected.

How the tier picks an element: Ghost draws numbered marks on the detected
elements and asks the model which number matches, then clicks that element's
exact rectangle, rather than asking the model to guess coordinates (which
landed about 250 px off in testing). For canvas apps with no accessibility tree
it adds boxes from a CPU edge detector; an optional OmniParser ONNX tier
(`--features yolo` and `GHOST_YOLO_MODEL`) plugs into the same path.

## HTTP server

```bash
ghost-http --addr 127.0.0.1:7878
```

`--addr` must be a loopback address. Endpoints: `/health`, `/tools`, `/click`,
`/click-at`, `/type`, `/press`, `/hotkey`, `/screenshot`, `/launch`,
`/list-windows`, `/focus-window`, `/window-state`, `/describe`, `/clipboard`
(GET/POST), `/run` (an intent; `Content-Type: application/json`).
