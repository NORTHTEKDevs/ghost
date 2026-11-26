#!/usr/bin/env python3
"""C7: verification of ghost_click_at (and a double_click re-check) on Ghost's
hidden desktop. The testbed is launched there with ghost_window op=launch, so
no input can reach any window on the human's desktop.

Controls, n each, coordinates from ghost_find on the testbed:
  increment - Increment button: title counter must rise by exactly 1
  note      - Note button: only a distant label changes (read via ghost_see)
  label     - the static "Field" label: no effect, must read verified=false
  dbl       - ghost_act double_click on Increment: counter +2, verified

  python hidden_click_at_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 20 --out <dir>
"""
import argparse
import json
import os
import re
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, clicks, data_of, pct  # noqa: E402

T = "Ghost Hidden ClickAt Bed"


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
    env, err, _ = c.call("ghost_window", {"op": "launch", "exe": f'"{a.testbed}" --title "{T}"'})
    assert (data_of(env) or {}).get("surface") == "hidden", f"not on a hidden desktop: {env}"
    time.sleep(1.0)

    def title():
        env, _, _ = c.call("ghost_window", {"op": "list"})
        for w in data_of(env).get("windows", []):
            if (w.get("name") or "").startswith(T):
                return w["name"]
        return ""

    def notes():
        env, _, _ = c.call("ghost_see", {"mode": "text", "window": T})
        m = re.search(r"Status: noted (\d+)", json.dumps(data_of(env)))
        return int(m.group(1)) if m else 0

    def centre(name, role, idx=None):
        q = {"name": name, "role": role, "window": T}
        if idx is not None:
            q["index"] = idx
        env, _, _ = c.call("ghost_find", q)
        d = data_of(env)
        return d["center"]["x"], d["center"]["y"]

    pts = {"increment": centre("Increment", "button", 0), "note": centre("Note", "button"),
           "label": centre("Field", "text")}
    res = {}
    for ctl in ("increment", "note", "label", "dbl"):
        rows = []
        for _ in range(a.n):
            t0, n0 = title(), notes() if ctl == "note" else 0
            if ctl == "dbl":
                env, err, dt = c.call("ghost_act", {"action": "double_click", "name": "Increment", "role": "button",
                                                    "index": 0, "window": T})
            else:
                x, y = pts[ctl]
                env, err, dt = c.call("ghost_click_at", {"x": x, "y": y, "window": T})
            time.sleep(0.1)
            t1 = title()
            d = data_of(env) if isinstance(data_of(env), dict) else {}
            if ctl == "increment":
                ok = clicks(t1) == clicks(t0) + 1
            elif ctl == "dbl":
                ok = clicks(t1) == clicks(t0) + 2
            elif ctl == "note":
                ok = notes() == n0 + 1 and t1 == t0
            else:
                ok = t1 == t0
            rows.append({"correct": ok, "verified": d.get("verified"), "verified_by": d.get("verified_by"),
                         "is_error": err, "ms": round(dt, 1)})
        res[ctl] = rows
    c.call("ghost_window", {"op": "state", "name": T, "state": "close"})
    c.close()
    with open(os.path.join(a.out, "hidden_click_at.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    for ctl, rows in res.items():
        lat = [r["ms"] for r in rows]
        print(f"{ctl:9} correct {sum(r['correct'] for r in rows)}/{a.n}, verified=true {sum(r['verified'] is True for r in rows)}, "
              f"verified=false {sum(r['verified'] is False for r in rows)}, errors {sum(r['is_error'] for r in rows)}, "
              f"median {round(statistics.median(lat), 1)} ms, p95 {round(pct(lat, 95), 1)} ms, "
              f"by {sorted(set(str(r['verified_by']) for r in rows))}")


if __name__ == "__main__":
    main()
