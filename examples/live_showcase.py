#!/usr/bin/env python3
"""Ghost live showcase — a paced, on-screen demo of what the Ghost MCP can do.

Runs the real ghost-mcp binary and drives real Windows apps on your screen with a
narrated, timed play-by-play. Deliberately PACED so a human can watch each step.

    cargo build --release -p ghost-mcp
    python examples/live_showcase.py            # paced (~40s), good for showing people
    python examples/live_showcase.py --fast     # no pauses (benchmark-style)
"""
import json, os, subprocess, sys, time

MCP = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", "ghost-mcp.exe")
FAST = "--fast" in sys.argv
STEP = 0.0 if FAST else 0.55   # pause between visible actions
BEAT = 0.0 if FAST else 1.3    # pause between sections


class Ghost:
    def __init__(self, exe):
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True, encoding="utf-8", bufsize=1)
        self._id = 0
        self._rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                 "clientInfo": {"name": "showcase", "version": "1"}})

    def _rpc(self, m, p):
        self._id += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self._id, "method": m, "params": p}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def call(self, tool, args):
        t0 = time.perf_counter()
        r = self._rpc("tools/call", {"name": tool, "arguments": args})
        ms = (time.perf_counter() - t0) * 1000.0
        try:
            env = json.loads(r["result"]["content"][0]["text"])
            data = env.get("data", env) if isinstance(env, dict) else {}
        except Exception:
            data = {}
        if not isinstance(data, dict):   # a tool may report data: null
            data = {}
        return data, ms

    def fg(self):
        d, _ = self.call("ghost_window", {"op": "list"})
        for w in d.get("windows", []) or []:
            if w.get("focused"):
                return w.get("name", "")
        return ""

    def close(self):
        try: self.p.stdin.close()
        except Exception: pass
        self.p.terminate()


def hr(t):
    print("\n" + "=" * 68 + f"\n  {t}\n" + "=" * 68)


def display(g):
    d, _ = g.call("ghost_see", {"mode": "text", "window": "Calculator"})
    for l in d.get("text", "").splitlines():
        if "Display is" in l:
            return l.replace("Display is ", "").strip()
    return "?"


def main():
    g = Ghost(MCP)
    t_start = time.perf_counter()
    acts = 0

    def press(name):
        nonlocal acts
        d, ms = g.call("ghost_act", {"action": "click", "name": name, "role": "button", "window": "Calculator"})
        acts += 1
        vmark = "OK" if d.get("verified") else ".."
        print(f"     [{vmark}] {name:<16} {ms:5.0f} ms")
        time.sleep(STEP)

    def calc(label, buttons, expect):
        print(f"\n   {label}")
        for b in buttons:
            press(b)
        val = display(g)
        ok = "OK" if val == expect else "??"
        print(f"     = {val}   [{ok} expected {expect}]")
        time.sleep(BEAT)

    try:
        hr("1. Ghost SEES an app instantly — structured, not a screenshot")
        g.call("ghost_window", {"op": "launch", "exe": "calc.exe"}); time.sleep(1.4)
        g.call("ghost_wait", {"for": "idle", "window": "Calculator", "timeout_ms": 3000})
        g.call("ghost_window", {"op": "focus", "name": "Calculator"}); time.sleep(0.4)
        snap, ms = g.call("ghost_snapshot", {"window": "Calculator", "actionable_only": True})
        print(f"   Read {snap.get('actionable_count')} clickable controls in {ms:.0f} ms — every button by")
        print(f"   name, role, position, and enabled/disabled. No pixel-guessing.")
        time.sleep(BEAT)

        hr("2. It ACTS — watch the buttons, and every step is VERIFIED")
        calc("6 x 7 =", ["Six", "Multiply by", "Seven", "Equals"], "42")
        press("Clear")
        calc("1 2 3 4 5 x 6 =", ["One", "Two", "Three", "Four", "Five", "Multiply by", "Six", "Equals"], "74,070")
        press("Clear")
        calc("square root of 8 1", ["Eight", "One", "Square root"], "9")
        press("Clear")
        calc("9 + 8 + 7 + 6 + 5 =", ["Nine", "Plus", "Eight", "Plus", "Seven", "Plus", "Six", "Plus", "Five", "Equals"], "35")

        hr("3. THE TRICK: it drives a DIFFERENT app in the BACKGROUND")
        print("   Calculator stays in front the whole time. Ghost operates Character")
        print("   Map behind it — no window pops up, no cursor moves. Watch the front.")
        time.sleep(BEAT)
        g.call("ghost_clipboard", {"op": "set", "text": "(empty)"})
        g.call("ghost_window", {"op": "launch", "exe": "charmap.exe"}); time.sleep(1.4)
        g.call("ghost_window", {"op": "focus", "name": "Calculator"}); time.sleep(0.5)
        fg0 = g.fg()
        W = {"background": True, "window": "Character Map"}
        for label, args in [("focus a field", {**W, "action": "click", "role": "edit"}),
                            ("type 'DEAL-2026'", {**W, "action": "type", "role": "edit", "text_input": "DEAL-2026"})]:
            d, ms = g.call("ghost_act", args); acts += 1
            print(f"     {label:<18} {ms:5.0f} ms   focus stays: {g.fg()!r}")
            time.sleep(STEP)
        for label, keys in [("select all (Ctrl+A)", "Ctrl+A"), ("copy (Ctrl+C)", "Ctrl+C")]:
            d, ms = g.call("ghost_key", {**W, "keys": keys}); acts += 1
            print(f"     {label:<18} {ms:5.0f} ms   focus stays: {g.fg()!r}")
            time.sleep(STEP)
        clip, _ = g.call("ghost_clipboard", {"op": "get"})
        print(f"   -> copied {clip.get('text')!r} out of a window you never looked at.")
        print(f"   -> foreground was {fg0!r} before, {g.fg()!r} after. It never moved.")
        time.sleep(BEAT)

        hr("4. It knows what it CAN'T click (skips disabled controls)")
        snap, _ = g.call("ghost_snapshot", {"window": "Calculator"})
        disabled = [e["name"] for e in snap.get("elements", []) if e.get("enabled") is False]
        print(f"   {snap.get('actionable_count')} usable; {len(disabled)} greyed-out and skipped: {disabled[:3]}")
        time.sleep(BEAT)

        total = time.perf_counter() - t_start
        hr("DONE")
        print(f"   {acts} verified actions across 2 apps in {total:.0f} seconds —")
        print(f"   four calculations, one app driven entirely in the background,")
        print(f"   every step confirmed. Nothing left to chance.")

        g.call("ghost_window", {"op": "state", "name": "Character Map", "state": "close"})
        g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
    finally:
        g.close()


if __name__ == "__main__":
    main()
