#!/usr/bin/env python3
"""The anchored target closes while a window whose title contains its title is
still open. Does input go to that decoy?

Both windows are testbeds launched here. For each trial: launch target + decoy,
anchor the target, focus the decoy's edit through Ghost (so a misrouted key
would visibly land), close the target, then send `ghost_key q`:
  anchor   - no `window` argument: the call relies on the anchor
  explicit - window="<T>": the MCP target resolver re-matches the title
The decoy's edit is read with WM_GETTEXT (a read, no input).

  python target_closed_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
"""
import argparse
import ctypes
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from key_routing_probe import edit_value  # noqa: E402
from latency_probe import Client, data_of  # noqa: E402
from review_probes import launch_user  # noqa: E402

T = "Ghost Gone Bed"
DECOY = T + " 2"


def handles(c):
    env, _, _ = c.call("ghost_window", {"op": "list"})
    return {w.get("name"): w.get("hwnd") for w in data_of(env).get("windows", []) if (w.get("name") or "").startswith(T)}


def trial(exe, testbed, mode):
    c = Client(exe)
    c.init()
    pt = launch_user(testbed, T)
    time.sleep(0.8)
    pd = launch_user(testbed, DECOY)
    time.sleep(1.0)
    h = handles(c)
    c.call("ghost_act", {"action": "click", "role": "edit", "window": DECOY})
    c.call("ghost_window", {"op": "anchor", "name": T})
    c.call("ghost_window", {"op": "state", "name": T, "state": "close"})
    try:
        pt.wait(timeout=5)
    except Exception:
        pt.terminate()
    time.sleep(0.3)
    gone = not ctypes.windll.user32.IsWindow(h[T])
    args = {"keys": "q"} if mode == "anchor" else {"keys": "q", "window": T}
    env, err, _ = c.call("ghost_key", args)
    time.sleep(0.2)
    decoy_text = edit_value(h[DECOY])
    c.call("ghost_window", {"op": "state", "name": DECOY, "state": "close"})
    try:
        pd.wait(timeout=5)
    except Exception:
        pd.terminate()
    c.close()
    d = data_of(env)
    return {"target_gone": gone, "is_error": err, "decoy_got_q": "q" in decoy_text,
            "error": (env.get("error") if err else None),
            "sent_to": (d.get("window") if isinstance(d, dict) else None)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=10)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    res = {mode: [trial(a.exe, a.testbed, mode) for _ in range(a.n)] for mode in ("anchor", "explicit")}
    with open(os.path.join(a.out, "target_closed.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    for mode, rows in res.items():
        print(f"{mode:8}: target gone {sum(r['target_gone'] for r in rows)}/{a.n}, refused {sum(r['is_error'] for r in rows)}/{a.n}, "
              f"decoy got the key {sum(r['decoy_got_q'] for r in rows)}/{a.n}, sent_to {sorted(set(str(r['sent_to']) for r in rows))}")
        errs = sorted(set(str(r['error'])[:140] for r in rows if r['error']))
        print("          errors:", ascii(errs))


if __name__ == "__main__":
    main()
