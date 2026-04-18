# Ghost HTTP + Autonomous Agent Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `ghost_http_get` and `ghost_http_post` MCP tools to ghost-mcp, then build an autonomous overnight agent (`ghost-agent/agent.py`) that fetches a task list from a URL, loops screenshot -> Claude Vision -> action until each task is done.

**Architecture:** reqwest is added to ghost-mcp for HTTP tools. The orchestrator is a standalone Python script that spawns ghost-mcp.exe, takes screenshots via `mss` (bypasses DXGI pipe issue), sends them to Claude with task context, parses the returned action JSON, and calls the appropriate ghost tool. Runs tasks sequentially until the list is exhausted.

**Tech Stack:** Rust + reqwest (ghost-mcp), Python 3 + anthropic SDK + mss (orchestrator), Claude claude-sonnet-4-6 vision

---

### Task 1: Add reqwest to workspace and ghost-mcp Cargo.toml

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/ghost-mcp/Cargo.toml`

**Step 1: Add reqwest to workspace deps**

In `/c/Users/Krist/projects/active/ghost/Cargo.toml`, add inside `[workspace.dependencies]`:

```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
```

**Step 2: Add reqwest to ghost-mcp crate deps**

In `crates/ghost-mcp/Cargo.toml`, add inside `[dependencies]`:

```toml
reqwest = { workspace = true }
```

**Step 3: Verify it compiles**

```bash
cd /c/Users/Krist/projects/active/ghost
cargo check -p ghost-mcp 2>&1 | tail -5
```

Expected: no errors (reqwest will download)

**Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/ghost-mcp/Cargo.toml
git commit -m "feat(ghost-mcp): add reqwest dependency for HTTP tools"
```

---

### Task 2: Add ghost_http_get and ghost_http_post handlers

**Files:**
- Modify: `crates/ghost-mcp/src/main.rs`

**Step 1: Add reqwest client field**

The `handle` function is stateless today (takes `&GhostSession`). HTTP needs a client. The simplest approach: create a `reqwest::Client` once via `lazy_static` / `std::sync::OnceLock` at the top of main.rs.

Add this import at the top of main.rs, after existing `use` statements:

```rust
use std::sync::OnceLock;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ghost-mcp/0.2.0")
            .build()
            .expect("failed to build reqwest client")
    })
}
```

**Step 2: Add handlers in the `handle` match block**

Insert before the final `_ => Err(...)` arm (line ~273 in main.rs, right before `_ => Err(format!("unknown method: {}", method))`):

```rust
        "ghost_http_get" => {
            let url = p["url"].as_str().ok_or("missing param: url")?;
            let headers_val = p["headers"].as_object();
            let mut req = http_client().get(url);
            if let Some(hdrs) = headers_val {
                for (k, v) in hdrs {
                    if let Some(vs) = v.as_str() {
                        req = req.header(k.as_str(), vs);
                    }
                }
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(json!({ "status": status, "body": body }))
        }
        "ghost_http_post" => {
            let url = p["url"].as_str().ok_or("missing param: url")?;
            let body = p["body"].as_str().unwrap_or("");
            let content_type = p["content_type"].as_str().unwrap_or("application/json");
            let headers_val = p["headers"].as_object();
            let mut req = http_client()
                .post(url)
                .header("Content-Type", content_type)
                .body(body.to_owned());
            if let Some(hdrs) = headers_val {
                for (k, v) in hdrs {
                    if let Some(vs) = v.as_str() {
                        req = req.header(k.as_str(), vs);
                    }
                }
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let resp_body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(json!({ "status": status, "body": resp_body }))
        }
```

**Step 3: Add tool schemas**

In `tools_schema()`, before the closing `])`, add:

```rust
        ,{ "name": "ghost_http_get",
          "description": "Make an HTTP GET request. Returns status code and response body as text.",
          "inputSchema": { "type": "object", "required": ["url"], "properties": {
              "url": { "type": "string", "description": "Full URL to fetch" },
              "headers": { "type": "object", "description": "Optional request headers as key-value pairs" }
          }}}
        ,{ "name": "ghost_http_post",
          "description": "Make an HTTP POST request with a string body. Returns status code and response body.",
          "inputSchema": { "type": "object", "required": ["url"], "properties": {
              "url": { "type": "string" },
              "body": { "type": "string", "description": "Request body string" },
              "content_type": { "type": "string", "description": "Content-Type header (default: application/json)" },
              "headers": { "type": "object", "description": "Additional headers" }
          }}}
```

**Step 4: Update the tool count test**

In the `tests` module, find:
```rust
assert_eq!(list.len(), 25, "expected 25 tools (24 original + ghost_reset)");
```
Change to:
```rust
assert_eq!(list.len(), 27, "expected 27 tools (25 original + ghost_http_get + ghost_http_post)");
```

Also update `tools_schema_contains_all_required_tools` to include the new tools:
```rust
for required in &["ghost_find","ghost_click","ghost_type","ghost_screenshot",
                  "ghost_press","ghost_hotkey","ghost_scroll","ghost_describe_screen",
                  "ghost_get_clipboard","ghost_set_clipboard","ghost_list_windows",
                  "ghost_stop","ghost_reset","ghost_wait","ghost_get_text",
                  "ghost_http_get","ghost_http_post"] {
```

**Step 5: Run tests**

```bash
cd /c/Users/Krist/projects/active/ghost
cargo test -p ghost-mcp 2>&1 | tail -20
```

Expected: all tests pass including updated count

**Step 6: Release build**

```bash
cargo build -p ghost-mcp --release -q
```

**Step 7: Smoke test http_get**

```bash
python3 -c "
import subprocess, json

EXE = r'C:\Users\Krist\projects\active\ghost\target\release\ghost-mcp.exe'
proc = subprocess.Popen([EXE], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)

def send(id, method, params=None):
    req = json.dumps({'id': id, 'method': method, 'params': params or {}}) + '\n'
    proc.stdin.write(req.encode()); proc.stdin.flush()
    return json.loads(proc.stdout.readline())

send(1, 'initialize')
r = send(2, 'ghost_http_get', {'url': 'https://httpbin.org/get'})
print('status:', r['result']['status'])
print('body snippet:', r['result']['body'][:100])
proc.terminate()
"
```

Expected: status 200, body starts with `{`

**Step 8: Commit**

```bash
git add crates/ghost-mcp/src/main.rs
git commit -m "feat(ghost-mcp): add ghost_http_get and ghost_http_post tools"
```

---

### Task 3: Build the ghost-agent orchestrator

**Files:**
- Create: `ghost-agent/agent.py`
- Create: `ghost-agent/requirements.txt`

**Step 1: Create requirements.txt**

```
anthropic>=0.40.0
mss>=9.0.0
Pillow>=10.0.0
```

Install:
```bash
pip install anthropic mss Pillow
```

**Step 2: Create ghost-agent/agent.py**

```python
"""
ghost-agent: autonomous overnight agent
Usage: python agent.py --tasks <url-or-file> [--dry-run]

Task list format (JSON array at URL or local file):
[
  {"id": "1", "title": "Create Next.js project", "detail": "Run: npx create-next-app@latest client-app"},
  {"id": "2", "title": "Install dependencies", "detail": "cd client-app && npm install tailwindcss ..."}
]
"""
import argparse, base64, json, subprocess, sys, time, io
import anthropic
import mss
import mss.tools
from PIL import Image

GHOST_EXE = r"C:\Users\Krist\projects\active\ghost\target\release\ghost-mcp.exe"
MODEL = "claude-sonnet-4-6"
MAX_LOOPS_PER_TASK = 30   # safety limit per task
LOOP_DELAY_S = 2          # seconds between vision loops


def start_ghost():
    proc = subprocess.Popen(
        [GHOST_EXE],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )
    _send(proc, 1, "initialize")
    return proc


def _send(proc, req_id, method, params=None):
    line = json.dumps({"id": req_id, "method": method, "params": params or {}}) + "\n"
    proc.stdin.write(line.encode())
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())


class Ghost:
    def __init__(self):
        self.proc = start_ghost()
        self._id = 10

    def _req(self, method, params=None):
        self._id += 1
        r = _send(self.proc, self._id, method, params)
        if "error" in r:
            raise RuntimeError(f"ghost error: {r['error']['message']}")
        return r.get("result", {})

    def screenshot_b64(self) -> str:
        """Capture screen via mss (avoids DXGI pipe issue), return base64 PNG."""
        with mss.mss() as sct:
            monitor = sct.monitors[1]  # primary monitor
            img = sct.grab(monitor)
            # Resize to 50% to reduce token cost
            pil = Image.frombytes("RGB", img.size, img.bgra, "raw", "BGRX")
            w, h = pil.size
            pil = pil.resize((w // 2, h // 2), Image.LANCZOS)
            buf = io.BytesIO()
            pil.save(buf, format="PNG", optimize=True)
            return base64.b64encode(buf.getvalue()).decode()

    def http_get(self, url, headers=None):
        return self._req("ghost_http_get", {"url": url, **({"headers": headers} if headers else {})})

    def execute(self, action: dict):
        tool = action.get("tool")
        params = action.get("params", {})
        if tool:
            self._req(tool, params)

    def close(self):
        self.proc.terminate()


SYSTEM_PROMPT = """You are an autonomous desktop automation agent.
You receive a screenshot of the current screen and a task to complete.
You must respond with ONLY a JSON object (no markdown, no explanation) in this exact format:

{
  "thought": "one sentence about what you see and what to do next",
  "tool": "ghost_tool_name",
  "params": { ... },
  "done": false
}

When the task is fully complete, respond with:
{
  "thought": "task is done",
  "tool": null,
  "params": {},
  "done": true
}

Available tools and their params:
- ghost_click_at: {x: int, y: int}
- ghost_hotkey: {modifiers: [str], key: str}  e.g. modifiers=["Ctrl"], key="t"
- ghost_press: {key: str}  e.g. "Enter", "Tab", "Escape"
- ghost_type: {name: str (optional), role: str (optional), text: str}
- ghost_set_clipboard: {text: str}  then follow with ghost_hotkey Ctrl+V to paste
- ghost_focus_window: {name: str}
- ghost_launch: {exe: str}
- ghost_wait: {ms: int}
- ghost_http_get: {url: str, headers: {}}
- ghost_scroll: {x: int, y: int, direction: "up"|"down", amount: int}

Rules:
- Always look at the screenshot carefully before acting
- Use ghost_set_clipboard + Ctrl+V for pasting long text (more reliable than ghost_type)
- If a terminal/command prompt is needed, open Windows Terminal via ghost_launch with exe="wt.exe"
- If you need to run a command, focus the terminal, use set_clipboard with the command, paste it, then press Enter
- Screen coordinates: top-left is (0,0). Resolution is approx 1456x816 (screenshot is 50% scale of full res)
- Multiply screenshot coordinates by 2 to get actual screen coordinates
"""


def ask_claude(client, task_title, task_detail, screenshot_b64, history):
    messages = history + [
        {
            "role": "user",
            "content": [
                {
                    "type": "image",
                    "source": {"type": "base64", "media_type": "image/png", "data": screenshot_b64}
                },
                {
                    "type": "text",
                    "text": f"TASK: {task_title}\nDETAIL: {task_detail}\n\nWhat is your next action?"
                }
            ]
        }
    ]
    resp = client.messages.create(
        model=MODEL,
        max_tokens=512,
        system=SYSTEM_PROMPT,
        messages=messages
    )
    text = resp.content[0].text.strip()
    # Strip markdown code fences if Claude adds them despite instructions
    if text.startswith("```"):
        text = text.split("```")[1]
        if text.startswith("json"):
            text = text[4:]
    return json.loads(text), messages + [{"role": "assistant", "content": text}]


def fetch_tasks(ghost, source: str) -> list:
    """Fetch task list from URL (via ghost_http_get) or local JSON file."""
    if source.startswith("http"):
        result = ghost.http_get(source)
        return json.loads(result["body"])
    else:
        with open(source) as f:
            return json.load(f)


def run_task(ghost, client, task, dry_run=False):
    title = task.get("title", "")
    detail = task.get("detail", "")
    print(f"\n--- TASK: {title} ---")
    print(f"    {detail}")

    history = []
    for loop in range(MAX_LOOPS_PER_TASK):
        screenshot = ghost.screenshot_b64()
        action, history = ask_claude(client, title, detail, screenshot, history)

        print(f"  [{loop+1}] {action.get('thought', '')}")
        print(f"       -> {action.get('tool')} {action.get('params', {})}")

        if action.get("done"):
            print(f"  DONE: {title}")
            return True

        if not dry_run and action.get("tool"):
            ghost.execute(action)

        time.sleep(LOOP_DELAY_S)

    print(f"  WARNING: hit loop limit for task: {title}")
    return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tasks", required=True, help="URL or local JSON file with task list")
    parser.add_argument("--dry-run", action="store_true", help="Print actions without executing")
    args = parser.parse_args()

    client = anthropic.Anthropic()
    ghost = Ghost()

    try:
        print("Fetching task list...")
        tasks = fetch_tasks(ghost, args.tasks)
        print(f"Got {len(tasks)} tasks")

        for i, task in enumerate(tasks):
            print(f"\n=== Task {i+1}/{len(tasks)} ===")
            run_task(ghost, client, task, dry_run=args.dry_run)
            time.sleep(1)

        print("\nAll tasks complete.")
    finally:
        ghost.close()


if __name__ == "__main__":
    main()
```

**Step 3: Commit**

```bash
git add ghost-agent/
git commit -m "feat: add ghost-agent autonomous orchestrator with Claude Vision loop"
```

---

### Task 4: Create a task list and dry-run smoke test

**Step 1: Create a sample task list file**

Create `ghost-agent/tasks-example.json`:

```json
[
  {
    "id": "1",
    "title": "Open Windows Terminal",
    "detail": "Launch Windows Terminal using ghost_launch with exe='wt.exe'. Wait for it to open."
  },
  {
    "id": "2",
    "title": "Navigate to projects directory",
    "detail": "Type 'cd C:\\Users\\Krist\\projects\\active' into the terminal and press Enter."
  }
]
```

**Step 2: Dry-run to verify Claude parses and plans correctly**

```bash
cd /c/Users/Krist/projects/active/ghost
python3 ghost-agent/agent.py --tasks ghost-agent/tasks-example.json --dry-run
```

Expected: prints task list, Claude returns valid JSON actions for each loop, no crashes

**Step 3: Commit**

```bash
git add ghost-agent/tasks-example.json
git commit -m "feat: add example task list for ghost-agent"
```

---

### Task 5: Wire up real client task list and run live

**Step 1: Create client task list**

Either:
- Put tasks at a URL the agent can GET (Notion API, Linear API, a hosted JSON file)
- Or create a local `ghost-agent/tasks-client.json` with the full build steps

Format each task with:
- `"title"`: short name (Claude sees this)
- `"detail"`: exact instruction (what to type, what to open, what command to run)

Be specific in `detail` - "run `npx create-next-app@latest` in Windows Terminal" is better than "create next app"

**Step 2: Run live (no --dry-run)**

```bash
python3 ghost-agent/agent.py --tasks ghost-agent/tasks-client.json
```

**Step 3: Monitor output**

The agent prints each loop's thought + action. Watch the first 2-3 tasks manually to confirm it's on track before leaving it overnight.

**Emergency stop:** Close the terminal running agent.py, or press Ctrl+C. Ghost's emergency stop (Ctrl+Alt+G hotkey) is also wired in the binary if the mouse gets stuck.

---

### Notes for overnight run

- Set Windows power plan to "High Performance" - no sleep
- Keep the screen on (Settings > Power > Screen timeout = Never) 
- If Claude gets confused mid-task, it will hit the 30-loop limit and move to the next task
- Review the terminal log in the morning - each action is printed
- The history list grows per task but resets between tasks (prevents context overflow)
