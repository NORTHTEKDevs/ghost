#!/usr/bin/env python3
"""C1: ghost_click_at without hwnd, anchored on a target, when the point is
covered by another window.

All windows are created here: target T and cover B are separate ghost-testbed
processes; P is T's own owned popup (opened through Ghost). Layout: T at
(120,120) and B 260 px to its right, both CREATED topmost by the testbed
(--topmost, --pos; B launched second so it is above T), shown without
activation, so no other window can sit between them.
  covered   - T's "Note" button, which lies under B (B's Increment is there)
  uncovered - T's "Increment" button
  popup     - the "Ping" button on T's owned popup P
SAFETY: before every click the window at the point is checked with
WindowFromPoint; if it is not T, B or P the click is NOT sent (counted as
skipped). Nothing is sent to any window this script did not create.

  python cover_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 20 --out <dir>
"""
import argparse
import ctypes
import json
import os
import re
import subprocess
import sys
import time
from ctypes import wintypes

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from latency_probe import Client, data_of  # noqa: E402

u = ctypes.windll.user32
u.WindowFromPoint.argtypes = [wintypes.POINT]
u.WindowFromPoint.restype = wintypes.HWND
u.GetAncestor.restype = wintypes.HWND
u.FindWindowW.restype = wintypes.HWND
u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint]
u.SetWindowPos.restype = wintypes.BOOL
u.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
u.GetWindowLongPtrW.restype = ctypes.c_ssize_t
T, B = "Ghost Cover Bed", "Ghost Cover Over"
HWND_TOPMOST, HWND_NOTOPMOST = wintypes.HWND(-1), wintypes.HWND(-2)
SWP_NOSIZE, SWP_NOMOVE, SWP_NOACTIVATE = 0x1, 0x2, 0x10


def rect(h):
    r = wintypes.RECT()
    u.GetWindowRect(h, ctypes.byref(r))
    return r.left, r.top, r.right, r.bottom


def root_at(x, y):
    return u.GetAncestor(u.WindowFromPoint(wintypes.POINT(x, y)), 2) or 0  # GA_ROOT


def title(h):
    b = ctypes.create_unicode_buffer(256)
    u.GetWindowTextW(h, b, 256)
    return b.value


def count(t, tag):
    m = re.search(rf"\[{tag}=(\d+)\]", t)
    return int(m.group(1)) if m else 0


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
    def launch_at(title, x, y):
        si = subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        si.wShowWindow = 4  # SW_SHOWNOACTIVATE
        return subprocess.Popen([a.testbed, "--title", title, "--topmost", "--pos", f"{x},{y}"], startupinfo=si)

    # Order matters: T, then T's popup, then B - so B is the last window to
    # enter the topmost band and sits above T and its popup.
    pt = launch_at(T, 120, 120)
    time.sleep(1.0)
    c.call("ghost_window", {"op": "anchor", "name": T})
    c.call("ghost_act", {"action": "click", "name": "Popup", "role": "button", "window": T})
    time.sleep(0.5)
    pb = launch_at(B, 380, 120)
    time.sleep(1.0)
    env, _, _ = c.call("ghost_window", {"op": "list"})
    h = {w["name"]: w["hwnd"] for w in data_of(env)["windows"] if w.get("name") in (T, B)}
    ht, hb = h[T], h[B]
    hp = u.FindWindowW("GhostTestbedPopup", None) or 0
    topmost = lambda hh: bool(u.GetWindowLongPtrW(hh, -20) & 0x8)  # GWL_EXSTYLE, WS_EX_TOPMOST  # noqa: E731
    print(f"setup: topmost T={topmost(ht)} B={topmost(hb)} popup={topmost(hp) if hp else None}; "
          f"T at {rect(ht)[:2]}, B at {rect(hb)[:2]}")
    pl, ptop, _, _ = rect(hp) if hp else (0, 0, 0, 0)

    def centre(name, idx=0):
        env, _, _ = c.call("ghost_find", {"name": name, "role": "button", "index": idx, "window": T})
        d = data_of(env)
        return d["center"]["x"], d["center"]["y"]

    points = {"covered": centre("Note"), "uncovered": centre("Increment"),
              "popup": (pl + 1 + 20 + 90, ptop + 1 + 15 + 20) if hp else None}
    expect_owner = {"covered": hb, "uncovered": ht, "popup": hp}
    c.call("ghost_window", {"op": "anchor", "name": T})  # ghost_find re-anchored on T anyway
    res = {"handles": {"target": ht, "cover": hb, "popup": hp}, "points": points}
    for case, pnt in points.items():
        rows = []
        for _ in range(a.n):
            if not pnt:
                rows.append({"skipped": "no popup"})
                continue
            owner = root_at(*pnt)
            if owner not in (ht, hb, hp) or owner != expect_owner[case]:
                rows.append({"skipped": f"point owned by {owner} {title(owner)!r}, not this script's expected window"})
                continue
            t0, b0 = title(ht), title(hb)
            env, err, dt = c.call("ghost_click_at", {"x": pnt[0], "y": pnt[1]})
            time.sleep(0.15)
            t1, b1 = title(ht), title(hb)
            d = data_of(env)
            rows.append({
                "refused": err, "error": (env.get("error") or "")[:240] if err else None,
                "target_clicks": count(t1, "clicks") - count(t0, "clicks"),
                "target_pings": count(t1, "popup") - count(t0, "popup"),
                "target_notes_title_unchanged": t0 == t1,
                "cover_clicks": count(b1, "clicks") - count(b0, "clicks"),
                "cover_changed": b0 != b1,
                "verified": d.get("verified") if isinstance(d, dict) else None, "ms": round(dt, 1),
            })
        res[case] = rows
    for name, proc in ((B, pb), (T, pt)):
        c.call("ghost_window", {"op": "state", "name": name, "state": "close"})
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.terminate()
    c.close()
    with open(os.path.join(a.out, "cover.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    for case in ("covered", "uncovered", "popup"):
        rows = [r for r in res[case] if "skipped" not in r]
        sk = len(res[case]) - len(rows)
        n = len(rows)
        print(f"{case:9} sent {n}, skipped {sk}: refused {sum(r['refused'] for r in rows)}, "
              f"cover clicked {sum(r['cover_clicks'] for r in rows)}, target clicks {sum(r['target_clicks'] for r in rows)}, "
              f"popup pings {sum(r['target_pings'] for r in rows)}, verified {sum(r['verified'] is True for r in rows)}")
        errs = sorted(set(r["error"][:160] for r in rows if r.get("error")))
        if errs:
            print("          error:", ascii(errs))
        if sk:
            print("          skip:", ascii(sorted(set(r["skipped"] for r in res[case] if "skipped" in r))))


def close_own_windows():
    """Close this script's testbeds (matched by testbed class AND exact title)
    whatever happened - they are topmost and must not stay over the screen."""
    for t in (T, B):
        for _ in range(3):
            h = u.FindWindowW("GhostTestbedWindow", t)
            if not h:
                break
            u.PostMessageW(h, 0x0010, 0, 0)  # WM_CLOSE
            time.sleep(0.3)


if __name__ == "__main__":
    try:
        main()
    finally:
        close_own_windows()
