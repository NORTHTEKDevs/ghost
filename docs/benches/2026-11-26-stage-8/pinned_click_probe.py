#!/usr/bin/env python3
"""C4: ghost_click_at pinned to the testbed's own hwnd (posted to it directly -
no WindowFromPoint, so no other window can receive it).

Per call: counter delta (exactly 1 is right; 2 = posted click AND UIA Invoke),
whether the UIA-Invoke fallback note appears, verified / verified_by, latency,
and whether the foreground window changed during the call (the fallback can
activate the target).

  python pinned_click_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 20 --out <dir>
"""
import argparse
import ctypes
import json
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, clicks, data_of, pct  # noqa: E402
from review_probes import close, launch_user  # noqa: E402

T = "Ghost Pinned Bed"
u = ctypes.windll.user32
u.GetForegroundWindow.restype = ctypes.c_void_p


def title(h):
    b = ctypes.create_unicode_buffer(256)
    u.GetWindowTextW(h, b, 256)
    return b.value


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("--control", choices=["increment", "label"], default="increment",
                    help="label: the static 'Field' label - a click with no effect, must read verified=false")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    c = Client(a.exe)
    c.init()
    p = launch_user(a.testbed, T)
    time.sleep(1.0)
    target = {"increment": {"name": "Increment", "role": "button", "index": 0},
              "label": {"name": "Field", "role": "text"}}[a.control]
    env, _, _ = c.call("ghost_find", {**target, "window": T})
    d = data_of(env)
    cx, cy = d["center"]["x"], d["center"]["y"]
    hwnd = (env.get("target") or d.get("target") or {}).get("hwnd")
    assert hwnd, "no testbed hwnd: refusing to click by coordinates alone"
    rows = []
    for _ in range(a.n):
        fg0 = u.GetForegroundWindow()
        before = clicks(title(hwnd))
        env, err, dt = c.call("ghost_click_at", {"x": cx, "y": cy, "hwnd": hwnd})
        time.sleep(0.15)
        fg1 = u.GetForegroundWindow()
        dd = data_of(env) if isinstance(data_of(env), dict) else {}
        rows.append({"delta": clicks(title(hwnd)) - before, "verified": dd.get("verified"),
                     "verified_by": dd.get("verified_by"), "fallback": "UIA Invoke" in str(dd.get("note")),
                     "foreground_changed": fg0 != fg1, "focus_preserved": dd.get("focus_preserved"),
                     "is_error": err, "ms": round(dt, 1)})
    close(c, [p], [T])
    c.close()
    lat = [r["ms"] for r in rows]
    s = {"n": a.n, "landed_once": sum(r["delta"] == 1 for r in rows), "double": sum(r["delta"] >= 2 for r in rows),
         "missed": sum(r["delta"] == 0 for r in rows), "fallback": sum(r["fallback"] for r in rows),
         "verified": sum(r["verified"] is True for r in rows),
         "foreground_changed": sum(r["foreground_changed"] for r in rows),
         "median_ms": round(statistics.median(lat), 1), "p95_ms": round(pct(lat, 95), 1), "raw": rows}
    with open(os.path.join(a.out, "pinned_click.json"), "w", encoding="utf-8") as f:
        json.dump(s, f, indent=1)
    print(f"pinned click_at [{a.control}] n={a.n}: landed once {s['landed_once']}, twice+ {s['double']}, missed {s['missed']}, "
          f"fallback {s['fallback']}, verified {s['verified']}, foreground changed {s['foreground_changed']}, "
          f"median {s['median_ms']} ms, p95 {s['p95_ms']} ms, verified_by {sorted(set(str(r['verified_by']) for r in rows))}")


if __name__ == "__main__":
    main()
