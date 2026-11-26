#!/usr/bin/env python3
"""Does headless Chrome starve for frames without a vsync source?

Launches the browser with Ghost's exact headless command line
(crates/ghost-browser/src/launch.rs base_args), optionally plus frame flags,
opens the tab the way Ghost does (Target.createTarget background:true), and
times the capture Ghost performs (Page.bringToFront + Page.captureScreenshot).
Also measures the browser's CPU while the page sits idle, since free-running
frames could cost CPU.

  python flag_variants.py --chrome <chrome.exe> --n 20 --out <dir>
"""
import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import psutil

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp_variants import Cdp  # noqa: E402
from latency_probe import pct  # noqa: E402
from tab_shot_probe import PAGE, check  # noqa: E402

GHOST_HEADLESS = [
    "--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check",
    "--disable-session-crashed-bubble", "--disable-popup-blocking",
    "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding", "--window-size=1280,900", "--headless=new",
]
VARIANTS = [
    ("A: ghost headless as shipped", []),
    ("B: + --disable-gpu-vsync --disable-frame-rate-limit", ["--disable-gpu-vsync", "--disable-frame-rate-limit"]),
    ("C: + --disable-frame-rate-limit", ["--disable-frame-rate-limit"]),
    ("D: + --disable-gpu-vsync", ["--disable-gpu-vsync"]),
    ("E: + --disable-gpu", ["--disable-gpu"]),
    ("F: + --disable-gpu-compositing", ["--disable-gpu-compositing"]),
    ("G: + --run-all-compositor-stages-before-draw", ["--run-all-compositor-stages-before-draw"]),
    ("H: + --disable-features=CalculateNativeWinOcclusion", ["--disable-features=CalculateNativeWinOcclusion"]),
]


def tree_cpu_seconds(proc):
    total = 0.0
    for p in [proc] + proc.children(recursive=True):
        try:
            t = p.cpu_times()
            total += t.user + t.system
        except psutil.Error:
            pass
    return total


def run_variant(chrome, extra, n, settle_s=0.0, idle_s=5.0):
    profile = tempfile.mkdtemp(prefix="ghost-flagprobe-")
    proc = subprocess.Popen([chrome, f"--user-data-dir={profile}", *GHOST_HEADLESS, *extra, "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        port_file = os.path.join(profile, "DevToolsActivePort")
        for _ in range(100):
            if os.path.exists(port_file) and open(port_file).read().strip():
                break
            time.sleep(0.05)
        port, path = open(port_file).read().split()[:2]
        cdp = Cdp(f"ws://127.0.0.1:{port}{path}")
        target = cdp.call("Target.createTarget", {"url": PAGE, "background": True})["targetId"]
        sess = cdp.call("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
        time.sleep(0.5)
        rows = []
        for _ in range(n):
            t0 = time.perf_counter()
            cdp.call("Page.bringToFront", {}, sess)
            b64 = cdp.call("Page.captureScreenshot", {"format": "png"}, sess)["data"]
            dt = (time.perf_counter() - t0) * 1000
            ok, centre, px, size = check(b64)
            rows.append({"ms": round(dt, 1), "ok": ok, "centre_ok": centre})
        ps = psutil.Process(proc.pid)
        time.sleep(settle_s)
        c0 = tree_cpu_seconds(ps)
        time.sleep(idle_s)
        idle_cpu_pct = (tree_cpu_seconds(ps) - c0) / idle_s * 100
        lat = [r["ms"] for r in rows]
        return {"n": n, "first_ms": lat[0], "median_ms": round(statistics.median(lat), 1),
                "p95_ms": round(pct(lat, 95), 1), "valid": sum(r["ok"] for r in rows),
                "centre_ok": sum(r["centre_ok"] for r in rows), "idle_cpu_pct_of_one_core": round(idle_cpu_pct, 1),
                "raw": rows}
    finally:
        for p in [psutil.Process(proc.pid)] + psutil.Process(proc.pid).children(recursive=True):
            try:
                p.kill()
            except psutil.Error:
                pass
        proc.wait(10)
        shutil.rmtree(profile, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--chrome", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", default="", help="comma-separated variant letters, e.g. E,F")
    ap.add_argument("--settle-s", type=float, default=0.0, help="wait after the captures before measuring CPU")
    ap.add_argument("--idle-s", type=float, default=5.0, help="idle CPU measurement window")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    lines = ["| launch flags | n | first ms | median ms | p95 ms | valid | page centre | idle CPU (% of a core) |",
             "|---|---|---|---|---|---|---|---|"]
    results = []
    only = set(a.only.split(",")) if a.only else None
    for name, extra in VARIANTS:
        if only and name[0] not in only:
            continue
        r = run_variant(a.chrome, extra, a.n, a.settle_s, a.idle_s)
        r["variant"] = name
        results.append(r)
        lines.append(f"| {name} | {r['n']} | {r['first_ms']} | {r['median_ms']} | {r['p95_ms']} | "
                     f"{r['valid']}/{r['n']} | {r['centre_ok']}/{r['n']} | {r['idle_cpu_pct_of_one_core']} |")
    with open(os.path.join(a.out, "flags.json"), "w", encoding="utf-8") as f:
        json.dump(results, f, indent=1)
    md = "\n".join(lines) + "\n"
    with open(os.path.join(a.out, "flags.md"), "w", encoding="utf-8") as f:
        f.write(md)
    print(md)


if __name__ == "__main__":
    main()
