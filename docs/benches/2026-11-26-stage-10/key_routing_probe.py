#!/usr/bin/env python3
"""Stage 10 copy (adds latency and focus-guard lines). C2: do ghost_key and the background edit commands reach the anchored window
when another window's title contains the target's and sits in front of it?

Both windows are testbeds this script launches; no input can reach anything
else. "<T>" is launched first, "<T> 2" (the decoy) last, so the decoy is first
in the window list - the order that used to send the key to the decoy.

  python key_routing_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 20 --out <dir>
"""
import argparse
import ctypes
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, data_of  # noqa: E402
from review_probes import close, launch_user, titles  # noqa: E402

T = "Ghost Key Bed"
DECOY = T + " 2"


def edit_value(top_hwnd):
    """The EDIT control's text, read with WM_GETTEXT - not through Ghost, whose
    ghost_see mode=text re-resolves the window by title and, with this decoy in
    front, returned the decoy's text for the target (candidate C9)."""
    u = ctypes.windll.user32
    edit = u.FindWindowExW(top_hwnd, None, "Edit", None)
    buf = ctypes.create_unicode_buffer(4096)
    u.SendMessageW(edit, 0x000D, 4096, buf)  # WM_GETTEXT: a read, no input
    return buf.value


def hwnds(c):
    env, _, _ = c.call("ghost_window", {"op": "list"})
    d = data_of(env)
    return {w.get("name"): w.get("hwnd") for w in d.get("windows", []) if (w.get("name") or "").startswith(T)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    c = Client(a.exe)
    c.init()
    p1 = launch_user(a.testbed, T)
    time.sleep(0.8)
    p2 = launch_user(a.testbed, DECOY)
    time.sleep(1.0)
    order = [x for x in titles(c) if x.startswith(T)]
    handles = hwnds(c)
    c.call("ghost_window", {"op": "anchor", "name": T})
    c.call("ghost_act", {"action": "click", "role": "edit", "window": T})
    time.sleep(0.2)

    key_rows = []
    for _ in range(a.n):
        env, err, dt = c.call("ghost_key", {"keys": "q", "window": T})
        d = data_of(env)
        key_rows.append({"window": d.get("window") if isinstance(d, dict) else None, "is_error": err,
                         "ms": round(dt, 1),
                         "focus_preserved": d.get("focus_preserved") if isinstance(d, dict) else None,
                         "focus_guard": bool(d.get("focus_guard")) if isinstance(d, dict) else None})
    time.sleep(0.2)
    target_text, decoy_text = edit_value(handles[T]), edit_value(handles[DECOY])

    cmd_rows = []
    for _ in range(a.n):
        env, err, dt = c.call("ghost_key", {"keys": "Ctrl+A", "window": T, "background": True})
        d = data_of(env)
        cmd_rows.append({"window": d.get("window") if isinstance(d, dict) else None, "is_error": err,
                         "ms": round(dt, 1)})
    close(c, [p1, p2], [DECOY, T])
    c.close()

    res = {
        "window_order": order,
        "keys": {"n": a.n, "q_in_target": target_text.count("q"), "q_in_decoy": decoy_text.count("q"),
                 "response_window_is_target": sum(r["window"] == T for r in key_rows),
                 "errors": sum(r["is_error"] for r in key_rows), "raw": key_rows},
        "edit_command": {"n": a.n, "response_window_is_target": sum(r["window"] == T for r in cmd_rows),
                         "errors": sum(r["is_error"] for r in cmd_rows), "raw": cmd_rows},
    }
    with open(os.path.join(a.out, "key_routing.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    k, e = res["keys"], res["edit_command"]
    print(f"window order: {order}")
    print(f"ghost_key q x{a.n}: q in target edit {k['q_in_target']}, q in decoy edit {k['q_in_decoy']}, "
          f"response window = target {k['response_window_is_target']}/{a.n}, errors {k['errors']}")
    print(f"Ctrl+A (edit command) x{a.n}: response window = target {e['response_window_is_target']}/{a.n}, "
          f"errors {e['errors']}")
    ms = sorted(r["ms"] for r in k["raw"])
    print(f"ghost_key q latency: median {ms[len(ms) // 2]:.1f} ms, min {ms[0]:.1f}, max {ms[-1]:.1f}; "
          f"focus_preserved {sum(r['focus_preserved'] is True for r in k['raw'])}/{a.n}, "
          f"focus_guard fired {sum(bool(r['focus_guard']) for r in k['raw'])}/{a.n}")
    return 0 if k["q_in_target"] == a.n and k["q_in_decoy"] == 0 and e["response_window_is_target"] == a.n else 1


if __name__ == "__main__":
    sys.exit(main())
