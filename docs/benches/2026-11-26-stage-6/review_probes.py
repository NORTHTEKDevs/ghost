#!/usr/bin/env python3
"""Experiments behind the convergence-round candidates. Each testbed gets a unique
title so nothing else on the machine can answer to it.

E1 click_at: ghost_click_at on the Increment button's centre, user desktop,
   ALWAYS pinned to the testbed's hwnd. Count title clicks per call: 1 is right,
   2 means the posted click AND the UIA-Invoke fallback both fired.
E2 key routing: two windows "<T>" and "<T> 2"; the edit of "<T>" is focused and
   anchored; ghost_key "q" with window="<T>". Which edit received it?
E3 double_click verification on the user desktop: is the effect (2 clicks) seen?

  python review_probes.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, data_of  # noqa: E402


def launch_user(testbed, title):
    si = subprocess.STARTUPINFO()
    si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    si.wShowWindow = 4  # SW_SHOWNOACTIVATE
    return subprocess.Popen([testbed, "--title", title], startupinfo=si)


def titles(c):
    env, _, _ = c.call("ghost_window", {"op": "list"})
    d = data_of(env)
    wins = d.get("windows", d) if isinstance(d, dict) else d
    return [w.get("name") or w.get("title") or "" for w in (wins or [])]


def title_of(c, prefix):
    for t in titles(c):
        if t.startswith(prefix) and not t.startswith(prefix + " 2"):
            return t
    return ""


def clicks(t):
    m = re.search(r"\[clicks=(\d+)\]", t)
    return int(m.group(1)) if m else 0


def edit_text(c, window):
    env, _, _ = c.call("ghost_find", {"role": "edit", "window": window})
    env, _, _ = c.call("ghost_see", {"mode": "text", "window": window})
    return json.dumps(data_of(env))


def close(c, procs, names):
    for n in names:
        c.call("ghost_window", {"op": "state", "name": n, "state": "close"})
    for p in procs:
        try:
            p.wait(timeout=5)
        except Exception:
            p.terminate()


def e1_click_at(c, testbed, n):
    t = "Ghost ClickAt Bed"
    p = launch_user(testbed, t)
    time.sleep(1.0)
    env, _, _ = c.call("ghost_find", {"name": "Increment", "role": "button", "index": 0, "window": t})
    d = data_of(env)
    cx, cy = d["center"]["x"], d["center"]["y"]
    hwnd = (env.get("target") or d.get("target") or {}).get("hwnd")
    out = {}
    # SAFETY: only ever with the testbed's own hwnd. Without it, ghost_click_at
    # posts to WindowFromPoint(x, y) - whatever covers the testbed - and the
    # first run of this script delivered 10 clicks to the terminal hosting the
    # session (e1-covering-window.txt). Never re-add a "without_hwnd" case on
    # the user's desktop; use the hidden desktop or cover the testbed with a
    # second testbed launched by this script.
    assert hwnd, "no testbed hwnd: refusing to click by coordinates alone"
    for label, extra in (("with_hwnd", {"hwnd": hwnd}),):
        rows = []
        for _ in range(n):
            before = clicks(title_of(c, t))
            env, err, dt = c.call("ghost_click_at", {"x": cx, "y": cy, **extra})
            time.sleep(0.15)
            after = clicks(title_of(c, t))
            dd = data_of(env)
            rows.append({"delta": after - before, "verified": dd.get("verified") if isinstance(dd, dict) else None,
                         "note": (dd.get("note") if isinstance(dd, dict) else None), "is_error": err,
                         "ms": round(dt, 1)})
        out[label] = rows
    close(c, [p], [t])
    return out


def e2_key_routing(c, testbed, decoy_on_top=False):
    """Both windows are launched here; the key can only reach one of them."""
    t = "Ghost Key Bed"
    if decoy_on_top:
        p1 = launch_user(testbed, t)
        time.sleep(0.8)
        p2 = launch_user(testbed, t + " 2")  # last shown: first in the window list
    else:
        p2 = launch_user(testbed, t + " 2")
        time.sleep(0.8)
        p1 = launch_user(testbed, t)
    time.sleep(1.0)
    c.call("ghost_window", {"op": "anchor", "name": t})
    c.call("ghost_act", {"action": "click", "role": "edit", "window": t})
    time.sleep(0.2)
    env, err, _ = c.call("ghost_key", {"keys": "q", "window": t})
    time.sleep(0.2)
    got = {"target": "q" in edit_text(c, t), "other": "q" in edit_text(c, t + " 2"),
           "key_response": data_of(env), "is_error": err, "window_order": [x for x in titles(c) if "Key Bed" in x]}
    close(c, [p1, p2], [t + " 2", t])
    return got


def e3_double_click(c, testbed, n):
    t = "Ghost Dbl Bed"
    p = launch_user(testbed, t)
    time.sleep(1.0)
    rows = []
    for _ in range(n):
        before = clicks(title_of(c, t))
        env, err, dt = c.call("ghost_act", {"action": "double_click", "name": "Increment", "role": "button",
                                            "index": 0, "window": t})
        time.sleep(0.15)
        after = clicks(title_of(c, t))
        dd = data_of(env)
        rows.append({"delta": after - before, "verified": dd.get("verified") if isinstance(dd, dict) else None,
                     "is_error": err, "ms": round(dt, 1)})
    close(c, [p], [t])
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=10)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", default="", help="e2 = re-run the key-routing experiment alone")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    c = Client(a.exe)
    c.init()
    if a.only in ("e2", "e2-decoy-on-top"):
        e2 = e2_key_routing(c, a.testbed, decoy_on_top=a.only == "e2-decoy-on-top")
        c.close()
        with open(os.path.join(a.out, f"review-{a.only}.json"), "w", encoding="utf-8") as f:
            json.dump(e2, f, indent=1, default=str)
        print(f"E2 key: target got it={e2['target']} other got it={e2['other']} order={e2['window_order']}")
        print("   response:", json.dumps(e2["key_response"])[:300])
        return
    res = {"e1_click_at": e1_click_at(c, a.testbed, a.n), "e2_key_routing": e2_key_routing(c, a.testbed),
           "e3_double_click": e3_double_click(c, a.testbed, a.n)}
    c.close()
    with open(os.path.join(a.out, "review.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1, default=str)
    for k in ("with_hwnd",):
        rows = res["e1_click_at"][k]
        print(f"E1 click_at {k}: deltas {[r['delta'] for r in rows]} verified {[r['verified'] for r in rows]}")
        print(f"   notes: {sorted(set(str(r['note'])[:90] for r in rows))}")
    e2 = res["e2_key_routing"]
    print(f"E2 key: target got it={e2['target']} other got it={e2['other']} order={e2['window_order']}")
    rows = res["e3_double_click"]
    print(f"E3 double_click: deltas {[r['delta'] for r in rows]} verified {[r['verified'] for r in rows]}")


if __name__ == "__main__":
    main()
