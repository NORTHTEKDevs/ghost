#!/usr/bin/env python3
"""Time Page.captureScreenshot strategies on a tab Ghost itself launched and opened.

Ghost launches the headless browser and opens the tab (same flags, same
background-tab state as ghost_tab_screenshot sees); this script then attaches to
that tab over raw CDP and times each strategy n times, checking every image's
centre pixel against the page background.

  python cdp_variants.py --exe <ghost-mcp.exe> --n 20 --out <dir>
"""
import argparse
import base64
import io
import json
import os
import statistics
import sys
import time
import urllib.request

import websocket  # websocket-client

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, pct  # noqa: E402
from tab_shot_probe import PAGE, check  # noqa: E402


class Cdp:
    def __init__(self, ws_url):
        self.ws = websocket.create_connection(ws_url, timeout=15, suppress_origin=True)
        self.id = 0

    def call(self, method, params=None, session=None, timeout=15):
        self.id += 1
        msg = {"id": self.id, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.ws.send(json.dumps(msg))
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.ws.settimeout(max(0.1, deadline - time.time()))
            r = json.loads(self.ws.recv())
            if r.get("id") == self.id:
                if "error" in r:
                    raise RuntimeError(f"{method}: {r['error']}")
                return r.get("result", {})
        raise TimeoutError(method)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    g = Client(a.exe)
    g.init()
    env, _, _ = g.call("ghost_browser_launch", {"id": "v", "mode": "headless"})
    port = (env.get("data") or {}).get("port")
    env, _, _ = g.call("ghost_tab_open", {"browser": "v", "url": PAGE})
    tab = (env.get("data") or {}).get("tab")
    ver = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version"))
    cdp = Cdp(ver["webSocketDebuggerUrl"])
    sess = cdp.call("Target.attachToTarget", {"targetId": tab, "flatten": True})["sessionId"]

    def vis():
        r = cdp.call("Runtime.evaluate", {"expression": "document.visibilityState + '/' + document.hasFocus()",
                                          "returnByValue": True}, sess)
        return r["result"]["value"]

    def shot(extra=None):
        p = {"format": "png"}
        p.update(extra or {})
        return cdp.call("Page.captureScreenshot", p, sess)["data"]

    def viewport():
        r = cdp.call("Runtime.evaluate", {"expression": "[innerWidth, innerHeight]", "returnByValue": True}, sess)
        return r["result"]["value"]

    def current():
        cdp.call("Page.bringToFront", {}, sess)
        return shot()

    def override():
        w, h = viewport()
        cdp.call("Emulation.setDeviceMetricsOverride",
                 {"width": w, "height": h, "deviceScaleFactor": 1, "mobile": False}, sess)
        try:
            return shot()
        finally:
            cdp.call("Emulation.clearDeviceMetricsOverride", {}, sess)

    def front_optimize():
        cdp.call("Page.bringToFront", {}, sess)
        return shot({"optimizeForSpeed": True})

    def front_from_surface():
        cdp.call("Page.bringToFront", {}, sess)
        return shot({"fromSurface": True})

    def raf_then_shot():
        cdp.call("Page.bringToFront", {}, sess)
        cdp.call("Runtime.evaluate", {"expression": "new Promise(r => requestAnimationFrame(() => r(1)))",
                                      "awaitPromise": True}, sess)
        return shot()

    def focus_emulation():
        cdp.call("Page.bringToFront", {}, sess)
        cdp.call("Emulation.setFocusEmulationEnabled", {"enabled": True}, sess)
        return shot()

    def lifecycle_active():
        cdp.call("Page.bringToFront", {}, sess)
        cdp.call("Page.setWebLifecycleState", {"state": "active"}, sess)
        return shot()

    variants = [
        ("current: bringToFront + capture", current),
        ("metrics override toggle", override),
        ("bringToFront + optimizeForSpeed", front_optimize),
        ("bringToFront + fromSurface", front_from_surface),
        ("bringToFront + rAF await + capture", raf_then_shot),
        ("bringToFront + focus emulation", focus_emulation),
        ("bringToFront + lifecycle active", lifecycle_active),
        ("current again (order check)", current),
    ]
    out = {"visibility_before": vis(), "results": []}
    lines = [f"visibility/focus on attach: {out['visibility_before']}", "",
             "| strategy | n | first ms | median ms | p95 ms | valid | page centre | visibility after |",
             "|---|---|---|---|---|---|---|---|"]
    for name, fn in variants:
        rows = []
        for _ in range(a.n):
            t0 = time.perf_counter()
            try:
                b64 = fn()
                dt = (time.perf_counter() - t0) * 1000
                ok, centre, px, size = check(b64)
            except Exception as e:  # a strategy the browser rejects is a result, not a crash
                dt, ok, centre, px, size = (time.perf_counter() - t0) * 1000, False, False, str(e)[:120], None
            rows.append({"ms": round(dt, 1), "ok": ok, "centre_ok": centre, "px": px})
        lat = [r["ms"] for r in rows]
        r = {"strategy": name, "n": a.n, "first_ms": lat[0], "median_ms": round(statistics.median(lat), 1),
             "p95_ms": round(pct(lat, 95), 1), "valid": sum(x["ok"] for x in rows),
             "centre_ok": sum(x["centre_ok"] for x in rows), "visibility_after": vis(), "raw": rows}
        out["results"].append(r)
        lines.append(f"| {name} | {a.n} | {r['first_ms']} | {r['median_ms']} | {r['p95_ms']} | "
                     f"{r['valid']}/{a.n} | {r['centre_ok']}/{a.n} | {r['visibility_after']} |")
        # Leave no emulation state behind for the next strategy.
        for m, p in (("Emulation.setFocusEmulationEnabled", {"enabled": False}),):
            try:
                cdp.call(m, p, sess)
            except Exception:
                pass
    g.call("ghost_browser_close", {"id": "v"})
    g.close()
    with open(os.path.join(a.out, "variants.json"), "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, default=str)
    md = "\n".join(lines) + "\n"
    with open(os.path.join(a.out, "variants.md"), "w", encoding="utf-8") as f:
        f.write(md)
    print(md)


if __name__ == "__main__":
    main()
