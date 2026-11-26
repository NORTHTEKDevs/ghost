#!/usr/bin/env python3
"""ghost_see mode=delta after stage 9: per-window deltas of the resolved window.

Testbeds "<T>" (target, 120,120) and "<T> 2" (decoy, 700,400, launched last so
first in the window list); both launched here and the only windows that get
input (ghost_act clicks on their own "Count N" buttons, whose captions change).
Per trial, in each mode (explicit window="<T>", and anchored with no window):
  nochange - a delta right after a delta must be empty
  target   - click the target's Count: the next delta must report it
  decoy    - click the decoy's Count: the next delta of the target must be empty

  python delta_routing_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 10 --out <dir>
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


def changes(d):
    return sum(len(d.get(k) or []) for k in ("added", "removed", "updated"))


def mentions_count(d):
    return any(str(e.get("name", "")).startswith("Count ") for k in ("added", "removed", "updated") for e in d.get(k) or [])


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
    res = {}
    try:
        for mode in ("explicit", "anchored"):
            args = {"mode": "delta", "window": T} if mode == "explicit" else {"mode": "delta"}

            def delta(since):
                if mode == "anchored":
                    c.call("ghost_window", {"op": "anchor", "name": T})
                q = dict(args)
                if since is not None:
                    q["since_seq"] = since
                env, err, _ = c.call("ghost_see", q)
                d = data_of(env) if isinstance(data_of(env), dict) else {}
                return d, err

            rows = []
            d, _ = delta(None)
            seq = d.get("seq")
            for _ in range(a.n):
                d1, e1 = delta(seq)
                c.call("ghost_act", {"action": "click", "name": "Count", "role": "button", "window": T})
                time.sleep(0.2)
                d2, e2 = delta(d1.get("seq"))
                c.call("ghost_act", {"action": "click", "name": "Count", "role": "button", "window": DECOY})
                time.sleep(0.2)
                d3, e3 = delta(d2.get("seq"))
                seq = d3.get("seq")
                rows.append({
                    "nochange_empty": changes(d1) == 0 and not d1.get("baseline"),
                    "target_reported": mentions_count(d2) and not d2.get("baseline"),
                    "decoy_not_reported": changes(d3) == 0 and not d3.get("baseline"),
                    "errors": int(bool(e1)) + int(bool(e2)) + int(bool(e3)),
                    "seqs": [d1.get("seq"), d2.get("seq"), d3.get("seq")],
                })
            res[mode] = rows
    finally:
        close(c, [p1, p2], [DECOY, T])
        c.close()
    with open(os.path.join(a.out, "delta_routing.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, indent=1)
    ok = True
    for mode, rows in res.items():
        n = len(rows)
        s = {k: sum(r[k] for r in rows) for k in ("nochange_empty", "target_reported", "decoy_not_reported")}
        ok = ok and all(v == n for v in s.values())
        print(f"{mode:8}: no change -> empty {s['nochange_empty']}/{n}, target change reported {s['target_reported']}/{n}, "
              f"decoy change not reported {s['decoy_not_reported']}/{n}, errors {sum(r['errors'] for r in rows)}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
