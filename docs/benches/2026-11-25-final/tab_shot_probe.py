#!/usr/bin/env python3
"""Latency and correctness of ghost_tab_screenshot on a Ghost-owned headless browser.

  python tab_shot_probe.py --exe <ghost-mcp.exe> --n 20 --out <dir>
Exit 0 iff every capture returned a valid PNG that shows the page: the centre
pixel must be the page background #cc3333 (within 8 per channel).
"""
import argparse
import base64
import io
import json
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, pct  # noqa: E402
from PIL import Image  # noqa: E402

PAGE = "data:text/html,<body style='background:%23c33'><h1>probe</h1><button>b</button></body>"
PAGE_RGB = (0xCC, 0x33, 0x33)
PNG_MAGIC = bytes([0x89, 0x50, 0x4E, 0x47])


def check(b64):
    """(valid PNG, centre is the page background, centre pixel, image size)."""
    if not b64:
        return False, False, None, None
    raw = base64.b64decode(b64)
    if raw[:4] != PNG_MAGIC:
        return False, False, None, None
    img = Image.open(io.BytesIO(raw)).convert("RGB")
    px = img.getpixel((img.width // 2, img.height // 2))
    return True, all(abs(a - b) <= 8 for a, b in zip(px, PAGE_RGB)), px, img.size


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
        ok, centre_ok, px, size = check(b64) if not err else (False, False, None, None)
        rows.append({"ms": round(dt, 1), "ok": ok, "centre_ok": centre_ok, "centre_px": px, "size": size})
    c.call("ghost_browser_close", {"id": "p"})
    c.close()
    lat = [r["ms"] for r in rows]
    s = {"n": a.n, "first_ms": lat[0], "median_ms": round(statistics.median(lat), 1),
         "p95_ms": round(pct(lat, 95), 1), "valid_png": sum(r["ok"] for r in rows),
         "centre_ok": sum(r["centre_ok"] for r in rows), "raw": rows}
    with open(os.path.join(a.out, "tab_shot.json"), "w", encoding="utf-8") as f:
        json.dump(s, f, indent=1)
    print(f"tab screenshot n={a.n}: first {s['first_ms']} ms, median {s['median_ms']} ms, "
          f"p95 {s['p95_ms']} ms, valid PNG {s['valid_png']}/{a.n}, page centre {s['centre_ok']}/{a.n}, "
          f"size {rows[0]['size']}")
    return 0 if s["valid_png"] == a.n and s["centre_ok"] == a.n else 1


if __name__ == "__main__":
    sys.exit(main())
