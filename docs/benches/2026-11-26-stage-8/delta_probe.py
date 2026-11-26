#!/usr/bin/env python3
"""ghost_see mode=delta: which window does it read?

Testbeds "<T>" (target, 120,120) and "<T> 2" (decoy, 700,400, launched last so
first in the window list) - both launched here; the probe sends no input.
  explicit - mode=delta window="<T>": are the returned Increment buttons inside
             the target (left 120..640) or the decoy (left >= 700)?
  anchored - anchor the target, then mode=delta with no window: does it read
             the anchor? If not, it reads whatever window is in front; to keep
             the human's screen content out of the evidence, only booleans and
             counts are recorded - never element names or text.

  python delta_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
"""
import argparse
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, data_of  # noqa: E402
from review_probes import close  # noqa: E402

T = "Ghost Delta Bed"
DECOY = T + " 2"


def classify(env):
    """target / decoy / other / empty, from where the Increment buttons are."""
    d = data_of(env) if isinstance(data_of(env), dict) else {}
    els = d.get("elements") or d.get("added") or d.get("changed") or []
    if isinstance(d.get("snapshot"), dict):
        els = d["snapshot"].get("elements", els)
    lefts = []
    for e in els if isinstance(els, list) else []:
        r = e.get("rect") if isinstance(e.get("rect"), dict) else e
        if e.get("name") == "Increment" and r.get("left") is not None:
            lefts.append(r["left"])
    if not els:
        return "empty", 0
    if lefts and all(120 <= x < 640 for x in lefts):
        return "target", len(els)
    if lefts and all(x >= 700 for x in lefts):
        return "decoy", len(els)
    return "other", len(els)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=10)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    def launch(title, pos):
        si = subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        si.wShowWindow = 4  # SW_SHOWNOACTIVATE
        return subprocess.Popen([a.testbed, "--title", title, "--pos", pos], startupinfo=si)

    p1 = launch(T, "120,120")
    time.sleep(0.8)
    p2 = launch(DECOY, "700,400")
    time.sleep(1.0)
    c = Client(a.exe)
    c.init()
    res = {"explicit": [], "anchored": [], "shape": None}
    try:
        for _ in range(a.n):
            env, err, _ = c.call("ghost_see", {"mode": "delta", "window": T})
            if res["shape"] is None and isinstance(data_of(env), dict):
                res["shape"] = sorted(data_of(env).keys())
            kind, n = classify(env)
            res["explicit"].append({"reads": kind, "elements": n, "is_error": err})
        c.call("ghost_window", {"op": "anchor", "name": T})
        for _ in range(a.n):
            env, err, _ = c.call("ghost_see", {"mode": "delta"})
            kind, n = classify(env)
            res["anchored"].append({"reads": kind, "elements": n, "is_error": err})
    finally:
        close(c, [p1, p2], [DECOY, T])
        c.close()
    with open(os.path.join(a.out, "delta.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    print("reply keys:", res["shape"])
    for k in ("explicit", "anchored"):
        rows = res[k]
        counts = {x: sum(r["reads"] == x for r in rows) for x in ("target", "decoy", "other", "empty")}
        print(f"{k:8}: {counts}, errors {sum(r['is_error'] for r in rows)}")


if __name__ == "__main__":
    main()
