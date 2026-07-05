#!/usr/bin/env python3
"""Ghost background-agent demo — the thing no other Windows automation tool does.

You are using Calculator. Meanwhile an AI agent operates a DIFFERENT app
(Character Map) *entirely in the background* — clicking, typing, selecting,
copying — and your foreground never changes, your cursor never moves, and every
action comes back verified.

Run it:
    cargo build --release -p ghost-mcp
    python examples/background_agent_demo.py

Exit code 0 means every background action landed AND the foreground stayed yours,
so this doubles as a self-test of the background dispatch path.
"""
import json
import os
import subprocess
import sys
import time

MCP = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "target", "release", "ghost-mcp.exe")


class Ghost:
    """Tiny stdio JSON-RPC client for ghost-mcp (one process, serial calls)."""
    def __init__(self, exe):
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True, encoding="utf-8", bufsize=1)
        self._id = 0
        self._rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                 "clientInfo": {"name": "ghost-demo", "version": "1"}})

    def _rpc(self, method, params):
        self._id += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self._id,
                                       "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def call(self, tool, args):
        """Returns the tool's data envelope (the fields Ghost reports back)."""
        resp = self._rpc("tools/call", {"name": tool, "arguments": args})
        try:
            env = json.loads(resp["result"]["content"][0]["text"])
        except Exception:
            env = {}
        return env.get("data", env) if isinstance(env, dict) else {}

    def foreground(self):
        for w in self.call("ghost_window", {"op": "list"}).get("windows", []) or []:
            if w.get("focused"):
                return w.get("name", "")
        return ""

    def close(self):
        try: self.p.stdin.close()
        except Exception: pass
        self.p.terminate()


def line(step, detail, focus_ok, fg):
    mark = "OK " if focus_ok else "!! "
    print(f"  {mark}{step:<26} {detail:<34} focus_preserved={focus_ok}  foreground={fg!r}")


def main():
    g = Ghost(MCP)
    ok = True
    try:
        # Prime the clipboard so we can prove the agent's copy actually changed it.
        g.call("ghost_clipboard", {"op": "set", "text": "(nothing yet)"})

        # The app YOU are using, kept in the foreground the whole time.
        g.call("ghost_window", {"op": "launch", "exe": "calc.exe"}); time.sleep(1.5)
        # The app the AGENT drives — never brought to the front.
        g.call("ghost_window", {"op": "launch", "exe": "charmap.exe"}); time.sleep(1.5)
        g.call("ghost_window", {"op": "focus", "name": "Calculator"}); time.sleep(0.5)

        fg0 = g.foreground()
        print(f"\nYou are working in: {fg0!r}. The agent will now drive Character Map "
              f"in the background.\n")

        W = {"background": True, "window": "Character Map"}

        # 1. Focus a field in the background app (posts a click; no cursor moves).
        r = g.call("ghost_act", {**W, "action": "click", "role": "edit"})
        ok &= bool(r.get("focus_preserved")); line("click the text field", "", r.get("focus_preserved"), g.foreground())

        # 2. Type into it (WM_SETTEXT — the field updates without being visible).
        r = g.call("ghost_act", {**W, "action": "type", "role": "edit", "text_input": "invoice-4471"})
        ok &= bool(r.get("focus_preserved")); line("type 'invoice-4471'", f"verified={r.get('verified')}", r.get("focus_preserved"), g.foreground())

        # 3. Select all + copy — via semantic messages, reliable in the background.
        r = g.call("ghost_key", {**W, "keys": "Ctrl+A"})
        ok &= bool(r.get("focus_preserved")); line("Ctrl+A (select all)", "", r.get("focus_preserved"), g.foreground())
        r = g.call("ghost_key", {**W, "keys": "Ctrl+C"})
        ok &= bool(r.get("focus_preserved")); line("Ctrl+C (copy)", "", r.get("focus_preserved"), g.foreground())

        time.sleep(0.2)
        clip = g.call("ghost_clipboard", {"op": "get"}).get("text", "")
        fg1 = g.foreground()

        print(f"\nThe agent copied {clip!r} out of a window you never looked at.")
        print(f"Foreground was {fg0!r} before and {fg1!r} after — it never moved.\n")

        ok &= ("invoice-4471" in clip) and ("Calculator" in fg0) and ("Calculator" in fg1)
        print("RESULT:", "PASS — background agent worked, your screen stayed yours" if ok
              else "FAIL — see the timeline above")

        g.call("ghost_window", {"op": "state", "name": "Character Map", "state": "close"})
        g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
        return 0 if ok else 1
    finally:
        g.close()


if __name__ == "__main__":
    sys.exit(main())
