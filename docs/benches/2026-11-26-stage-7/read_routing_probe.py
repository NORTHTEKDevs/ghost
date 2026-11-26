#!/usr/bin/env python3
"""C9: do ghost_see's reads come from the window they report?

Testbeds "<T>" (target, launched first) and "<T> 2" (decoy, launched last, so
first in the window list) - both launched here, so the target's title is
contained in the decoy's. Through Ghost the target's edit gets "abq"; the decoy
is untouched. Then n times each, relying on the anchor (no `window` argument):
  text - ghost_see mode=text: does the text contain "abq"?
  tree - ghost_see (full):    does the reply contain "abq"?
Ground truth for the edit is WM_GETTEXT.

  python read_routing_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
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

T = "Ghost Read Bed"
DECOY = T + " 2"


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
    p1 = launch_user(a.testbed, T)
    time.sleep(0.8)
    p2 = launch_user(a.testbed, DECOY)
    time.sleep(1.0)
    env, _, _ = c.call("ghost_window", {"op": "list"})
    h = {w["name"]: w["hwnd"] for w in data_of(env)["windows"] if (w.get("name") or "").startswith(T)}
    c.call("ghost_window", {"op": "anchor", "name": T})
    c.call("ghost_act", {"action": "type", "role": "edit", "text_input": "abq", "window": T})
    time.sleep(0.3)
    truth = {"target_edit": edit_value(h[T]), "decoy_edit": edit_value(h[DECOY])}
    env, _, _ = c.call("ghost_window", {"op": "list"})
    titles_now = [w["name"] for w in data_of(env)["windows"] if (w.get("name") or "").startswith(T)]
    rows = {"text": [], "tree": []}
    for _ in range(a.n):
        env, err, dt = c.call("ghost_see", {"mode": "text"})
        body = json.dumps(data_of(env))
        tgt = (data_of(env) or {}).get("target") or env.get("target") or {}
        rows["text"].append({"reads_target": "abq" in body, "reported_hwnd": tgt.get("hwnd"),
                             "is_error": err, "ms": round(dt, 1)})
        env, err, dt = c.call("ghost_see", {})
        body = json.dumps(data_of(env))
        tgt = (data_of(env) or {}).get("target") or env.get("target") or {}
        rows["tree"].append({"reads_target": "abq" in body, "reported_hwnd": tgt.get("hwnd"),
                             "is_error": err, "ms": round(dt, 1)})
    close(c, [p1, p2], [DECOY, T])
    c.close()
    res = {"handles": h, "titles_after_setup": titles_now, "truth": truth, **rows}
    with open(os.path.join(a.out, "read_routing.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    print(f"truth: {truth}; titles: {titles_now}")
    for k in ("text", "tree"):
        r = rows[k]
        print(f"see {k:4}: reads the target {sum(x['reads_target'] for x in r)}/{a.n}, "
              f"reported hwnd = target {sum(x['reported_hwnd'] == h[T] for x in r)}/{a.n}, errors {sum(x['is_error'] for x in r)}")
    return 0 if all(x["reads_target"] for x in rows["text"]) else 1


if __name__ == "__main__":
    sys.exit(main())
