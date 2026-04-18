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
MAX_LOOPS_PER_TASK = 30
LOOP_DELAY_S = 2


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
        with mss.mss() as sct:
            monitor = sct.monitors[1]
            img = sct.grab(monitor)
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
    if text.startswith("```"):
        text = text.split("```")[1]
        if text.startswith("json"):
            text = text[4:]
    return json.loads(text), messages + [{"role": "assistant", "content": text}]


def fetch_tasks(ghost, source: str) -> list:
    if source.startswith("http"):
        result = ghost.http_get(source)
        return json.loads(result["body"])
    else:
        with open(source) as f:
            return json.load(f)


def run_task(ghost, client, task, dry_run=False, mock=False):
    title = task.get("title", "")
    detail = task.get("detail", "")
    print(f"\n--- TASK: {title} ---")
    print(f"    {detail}")

    history = []
    for loop in range(MAX_LOOPS_PER_TASK):
        if mock:
            action = {"thought": "mock: task looks done", "tool": None, "params": {}, "done": True}
            history = []
        else:
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
    parser.add_argument("--mock", action="store_true", help="Return canned actions instead of calling Claude (for testing)")
    args = parser.parse_args()

    client = anthropic.Anthropic() if not args.mock else None
    ghost = Ghost()

    try:
        print("Fetching task list...")
        tasks = fetch_tasks(ghost, args.tasks)
        print(f"Got {len(tasks)} tasks")

        for i, task in enumerate(tasks):
            print(f"\n=== Task {i+1}/{len(tasks)} ===")
            run_task(ghost, client, task, dry_run=args.dry_run, mock=args.mock)
            time.sleep(1)

        print("\nAll tasks complete.")
    finally:
        ghost.close()


if __name__ == "__main__":
    main()
