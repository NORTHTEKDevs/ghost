#!/usr/bin/env python3
"""Latency of ghost_tab_screenshot on a Ghost-owned headless browser.

  python tab_shot_probe.py --exe <ghost-mcp.exe> --n 20 --out <dir>
Exit 0 iff every capture returned a valid PNG.
"""
import argparse
import base64
import json
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, pct  # noqa: E402

PAGE = "data:text/html,<body style='background:%23c33'><h1>probe</h1><button>b</button></body>"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    c = Client(a.exe)
    c.init()
    c.call("ghost_browser_launch", {"id": "p", "mode": "headless"})
    env, _, _ = c.call("ghost_tab_open", {"browser": "p", "url": PAGE})
    tab = (env.get("data") or {}).get("tab")
    rows = []
    for _ in range(a.n):
        env, err, dt = c.call("ghost_tab_screenshot", {"browser": "p", "tab": tab})
        d = env.get("data") or env
        b64 = d.get("png_base64", "") if isinstance(d, dict) else ""
        ok = (not err) and base64.b64decode(b64[:16] + "==")[:4] == b"\x89PNG" if b64 else False
        rows.append({"ms": round(dt, 1), "ok": ok, "bytes": len(b64)})
    c.call("ghost_browser_close", {"id": "p"})
    c.close()
    lat = [r["ms"] for r in rows]
    s = {"n": a.n, "first_ms": lat[0], "median_ms": round(statistics.median(lat), 1),
         "p95_ms": round(pct(lat, 95), 1), "valid_png": sum(r["ok"] for r in rows), "raw": rows}
    with open(os.path.join(a.out, "tab_shot.json"), "w", encoding="utf-8") as f:
        json.dump(s, f, indent=1)
    print(f"tab screenshot n={a.n}: first {s['first_ms']} ms, median {s['median_ms']} ms, "
          f"p95 {s['p95_ms']} ms, valid PNG {s['valid_png']}/{a.n}")
    return 0 if s["valid_png"] == a.n else 1


if __name__ == "__main__":
    sys.exit(main())
