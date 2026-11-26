#!/usr/bin/env python3
"""Does how the tab is created decide how fast it can be captured?

Same launch as Ghost's headless browser (flag_variants.GHOST_HEADLESS); each
variant creates the tab differently, then times Ghost's capture
(Page.bringToFront + Page.captureScreenshot) n times.

  python target_variants.py --chrome <chrome.exe> --n 20 --out <dir>
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
import urllib.request

import psutil

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp_variants import Cdp  # noqa: E402
from flag_variants import GHOST_HEADLESS  # noqa: E402
from latency_probe import pct  # noqa: E402
from tab_shot_probe import PAGE, check  # noqa: E402


def make_tab(cdp, port, how):
    if how == "createTarget background:true (ghost today)":
        return cdp.call("Target.createTarget", {"url": PAGE, "background": True})["targetId"]
    if how == "createTarget background:false":
        return cdp.call("Target.createTarget", {"url": PAGE, "background": False})["targetId"]
    if how == "createTarget newWindow:true background:true":
        return cdp.call("Target.createTarget", {"url": PAGE, "newWindow": True, "background": True})["targetId"]
    if how == "the browser's initial tab, navigated":
        pages = [t for t in json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list"))
                 if t["type"] == "page"]
        tid = pages[0]["id"]
        s = cdp.call("Target.attachToTarget", {"targetId": tid, "flatten": True})["sessionId"]
        cdp.call("Page.navigate", {"url": PAGE}, s)
        return tid
    raise ValueError(how)


HOWS = [
    "createTarget background:true (ghost today)",
    "createTarget background:false",
    "createTarget newWindow:true background:true",
    "the browser's initial tab, navigated",
]


def run(chrome, how, n):
    profile = tempfile.mkdtemp(prefix="ghost-targetprobe-")
    proc = subprocess.Popen([chrome, f"--user-data-dir={profile}", *GHOST_HEADLESS, "about:blank"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        port_file = os.path.join(profile, "DevToolsActivePort")
        for _ in range(100):
            if os.path.exists(port_file) and open(port_file).read().strip():
                break
            time.sleep(0.05)
        port, path = open(port_file).read().split()[:2]
        cdp = Cdp(f"ws://127.0.0.1:{port}{path}")
        tid = make_tab(cdp, port, how)
        sess = cdp.call("Target.attachToTarget", {"targetId": tid, "flatten": True})["sessionId"]
        time.sleep(0.5)
        rows = []
        for _ in range(n):
            t0 = time.perf_counter()
            cdp.call("Page.bringToFront", {}, sess)
            b64 = cdp.call("Page.captureScreenshot", {"format": "png"}, sess)["data"]
            ok, centre, _, _ = check(b64)
            rows.append({"ms": round((time.perf_counter() - t0) * 1000, 1), "ok": ok, "centre_ok": centre})
        lat = [r["ms"] for r in rows]
        return {"how": how, "n": n, "first_ms": lat[0], "median_ms": round(statistics.median(lat), 1),
                "p95_ms": round(pct(lat, 95), 1), "valid": sum(r["ok"] for r in rows),
                "centre_ok": sum(r["centre_ok"] for r in rows), "raw": rows}
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
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    lines = ["| tab created by | n | first ms | median ms | p95 ms | valid | page centre |", "|---|---|---|---|---|---|---|"]
    res = []
    for how in HOWS:
        r = run(a.chrome, how, a.n)
        res.append(r)
        lines.append(f"| {how} | {r['n']} | {r['first_ms']} | {r['median_ms']} | {r['p95_ms']} | "
                     f"{r['valid']}/{r['n']} | {r['centre_ok']}/{r['n']} |")
    with open(os.path.join(a.out, "targets.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    md = "\n".join(lines) + "\n"
    with open(os.path.join(a.out, "targets.md"), "w", encoding="utf-8") as f:
        f.write(md)
    print(md)


if __name__ == "__main__":
    main()
