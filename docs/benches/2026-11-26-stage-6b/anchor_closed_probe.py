#!/usr/bin/env python3
"""C11: what does an unanchored verb do after the anchored window closes?

Both windows are testbeds launched here. Per trial: anchor the target, close it,
then (no `window` argument anywhere - each call relies on the anchor):
  1. ghost_key q          - must fail naming the closed anchor
  2. ghost_see mode=text  - must fail the same way, not read another window
then re-anchor on the decoy with window=<decoy> and
  3. ghost_see mode=text  - must read the decoy (its edit holds a marker)
Nothing is ever sent to a window this script did not launch. Step 2 on the old
build would read the human's foreground window, so it is not run there.

  python anchor_closed_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, data_of  # noqa: E402
from review_probes import launch_user  # noqa: E402

T = "Ghost Anchor Bed"
DECOY = "Ghost Other Bed"


def err_text(env):
    return str(env.get("error") or "")


def trial(exe, testbed):
    c = Client(exe)
    c.init()
    pt = launch_user(testbed, T)
    time.sleep(0.8)
    pd = launch_user(testbed, DECOY)
    time.sleep(1.0)
    c.call("ghost_act", {"action": "type", "role": "edit", "text_input": "decoymark", "window": DECOY})
    c.call("ghost_window", {"op": "anchor", "name": T})
    c.call("ghost_window", {"op": "state", "name": T, "state": "close"})
    try:
        pt.wait(timeout=5)
    except Exception:
        pt.terminate()
    time.sleep(0.3)
    k_env, k_err, _ = c.call("ghost_key", {"keys": "q"})
    s_env, s_err, _ = c.call("ghost_see", {"mode": "text"})
    c.call("ghost_see", {"mode": "text", "window": DECOY})  # explicit: re-anchors on the decoy
    r_env, r_err, _ = c.call("ghost_see", {"mode": "text"})
    c.call("ghost_window", {"op": "state", "name": DECOY, "state": "close"})
    try:
        pd.wait(timeout=5)
    except Exception:
        pd.terminate()
    c.close()
    names_anchor = lambda e: ("anchored window 'Ghost Anchor Bed'" in e) and ("closed" in e)  # noqa: E731
    return {
        "key_refused": k_err, "key_names_closed_anchor": names_anchor(err_text(k_env)), "key_error": err_text(k_env)[:220],
        "see_refused": s_err, "see_names_closed_anchor": names_anchor(err_text(s_env)), "see_error": err_text(s_env)[:220],
        "reanchored_read_is_decoy": (not r_err) and "decoymark" in json.dumps(data_of(r_env)),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=10)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    rows = [trial(a.exe, a.testbed) for _ in range(a.n)]
    with open(os.path.join(a.out, "anchor_closed.json"), "w", encoding="utf-8") as f:
        json.dump(rows, f, indent=1)
    n = a.n
    print(f"ghost_key after close: refused {sum(r['key_refused'] for r in rows)}/{n}, names the closed anchor {sum(r['key_names_closed_anchor'] for r in rows)}/{n}")
    print(f"ghost_see after close: refused {sum(r['see_refused'] for r in rows)}/{n}, names the closed anchor {sum(r['see_names_closed_anchor'] for r in rows)}/{n}")
    print(f"after re-anchoring on the decoy, unanchored read is the decoy: {sum(r['reanchored_read_is_decoy'] for r in rows)}/{n}")
    print("key error:", ascii(rows[0]["key_error"]))
    ok = all(r["key_names_closed_anchor"] and r["see_names_closed_anchor"] and r["reanchored_read_is_decoy"] for r in rows)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
