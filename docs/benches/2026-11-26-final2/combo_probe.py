#!/usr/bin/env python3
"""C10: what does ghost_key do with a modifier combo under the default
background policy (window given, no background flag)?

One testbed launched here; its edit is pre-filled and focused through Ghost,
then each combo is sent n times and the edit is read with WM_GETTEXT (a read,
no input) after every call. A bare letter appearing in the edit means the
modifiers were dropped.

  python combo_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from key_routing_probe import edit_value  # noqa: E402
from latency_probe import Client, data_of  # noqa: E402
from review_probes import close, launch_user  # noqa: E402

T = "Ghost Combo Bed"
PREFILL = "base"
# combo -> the bare key a modifier-dropping path would post
COMBOS = {"Ctrl+A": "a", "Ctrl+S": "s", "Ctrl+Shift+K": "k", "Alt+X": "x"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=10)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    c = Client(a.exe)
    c.init()
    p = launch_user(a.testbed, T)
    time.sleep(1.0)
    env, _, _ = c.call("ghost_window", {"op": "list"})
    hwnd = next(w["hwnd"] for w in data_of(env)["windows"] if w.get("name") == T)
    c.call("ghost_window", {"op": "anchor", "name": T})
    res = {}
    for combo, bare in COMBOS.items():
        rows = []
        for _ in range(a.n):
            c.call("ghost_act", {"action": "type", "role": "edit", "text_input": PREFILL, "window": T})
            c.call("ghost_act", {"action": "click", "role": "edit", "window": T})
            time.sleep(0.1)
            env, err, dt = c.call("ghost_key", {"keys": combo, "window": T})
            time.sleep(0.15)
            text = edit_value(hwnd)
            d = data_of(env)
            rows.append({"edit_after": text, "bare_key_typed": bare in text.replace(PREFILL, "", 1).lower(),
                         "is_error": err, "error": env.get("error") if err else None,
                         "command": d.get("command") if isinstance(d, dict) else None,
                         "key": d.get("key") if isinstance(d, dict) else None, "ms": round(dt, 1)})
        res[combo] = rows
    close(c, [p], [T])
    c.close()
    with open(os.path.join(a.out, "combos.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    bad = 0
    for combo, rows in res.items():
        typed = sum(r["bare_key_typed"] for r in rows)
        bad += typed
        print(f"{combo:13} bare key typed {typed}/{a.n}, refused {sum(r['is_error'] for r in rows)}/{a.n}, "
              f"as edit command {sum(bool(r['command']) for r in rows)}/{a.n}, posted key {sorted(set(str(r['key']) for r in rows))}")
        errs = sorted(set(str(r["error"])[:120] for r in rows if r["error"]))
        if errs:
            print("              error:", ascii(errs))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
