#!/usr/bin/env python3
"""Real-use latency + accuracy probe for ghost_see / ghost_find / ghost_act.

Drives a ghost-mcp binary over stdio (the transport an MCP client uses) against
the Ghost Testbed (crates/ghost-testbed) on Ghost's hidden desktop, under the
default background policy. Every accuracy check re-observes the world:
  - click: the testbed title gains "[clicks=N]"; N must rise by exactly 1
  - type:  the typed token must be read back from the window text
Also measures cold start (spawn -> initialize reply) and the tools/list payload.

  python latency_probe.py --exe <ghost-mcp.exe> --testbed <ghost-testbed.exe> --n 20 --out <dir>
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time

TITLE = "Ghost Probe Bed"


class Client:
    def __init__(self, exe):
        self.p = subprocess.Popen(
            [exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, encoding="utf-8",
            errors="replace", bufsize=1,
        )
        self._id = 0

    def rpc(self, method, params=None):
        self._id += 1
        req = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            req["params"] = params
        self.p.stdin.write(json.dumps(req) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout")
            msg = json.loads(line)
            if msg.get("id") == self._id:
                return msg, len(line)

    def init(self):
        return self.rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                       "clientInfo": {"name": "ghost-probe", "version": "1"}})

    def call(self, name, args):
        t0 = time.perf_counter()
        resp, _ = self.rpc("tools/call", {"name": name, "arguments": args})
        dt = (time.perf_counter() - t0) * 1000.0
        res = resp.get("result", {})
        env = {}
        try:
            env = json.loads(res["content"][0]["text"]) or {}
        except Exception:
            pass
        return env, bool(res.get("isError")), dt

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.terminate()


def pct(xs, p):
    s = sorted(xs)
    k = max(0, min(len(s) - 1, int(round(p / 100.0 * len(s) + 0.5)) - 1))
    return s[k]


def summary(name, rows):
    lat = [r["ms"] for r in rows]
    ok = [r for r in rows if r["correct"]]
    return {
        "op": name, "n": len(rows),
        "median_ms": round(statistics.median(lat), 1), "p95_ms": round(pct(lat, 95), 1),
        "min_ms": round(min(lat), 1), "max_ms": round(max(lat), 1),
        "correct": len(ok), "accuracy": round(len(ok) / len(rows), 3),
        "errors": sum(1 for r in rows if r["is_error"]),
        "verified_true": sum(1 for r in rows if r.get("verified") is True),
        "verified_null": sum(1 for r in rows if r.get("verified") is None),
        "verified_false": sum(1 for r in rows if r.get("verified") is False),
    }


def data_of(env):
    d = env.get("data")
    return d if d is not None else env


def window_title(c):
    env, _, _ = c.call("ghost_window", {"op": "list"})
    d = data_of(env)
    wins = d.get("windows", d) if isinstance(d, dict) else d
    for w in wins or []:
        t = w.get("name") or w.get("title") or ""
        if t.startswith(TITLE):
            return t
    return ""


def clicks(title):
    m = re.search(r"\[clicks=(\d+)\]", title)
    return int(m.group(1)) if m else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--testbed", required=True)
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("--surface", choices=["hidden", "user"], default="hidden")
    # increment: a click whose effect is the title. distant: a click whose only
    # effect is a label far from the button. disabled / label: negative controls
    # whose click must change nothing. caret-label: the label control right after
    # a click into the edit, so the edit owns focus (and a caret) during it.
    ap.add_argument("--action", choices=["click", "double_click"], default="click",
                    help="double_click: a real one must raise the counter by exactly 2")
    ap.add_argument("--control", choices=["increment", "distant", "disabled", "label", "caret-label"],
                    default="increment")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    out = {"exe": os.path.abspath(a.exe), "n": a.n, "ops": [], "raw": {}}

    # Cold start + tools/list payload.
    cold = []
    tl_bytes = tl_count = 0
    for _ in range(10):
        t0 = time.perf_counter()
        c = Client(a.exe)
        c.init()
        cold.append((time.perf_counter() - t0) * 1000.0)
        resp, nbytes = c.rpc("tools/list", {})
        tl_bytes, tl_count = nbytes, len(resp["result"]["tools"])
        c.close()
    out["cold_start"] = {"n": 10, "median_ms": round(statistics.median(cold), 1),
                         "p95_ms": round(pct(cold, 95), 1), "raw": [round(x, 1) for x in cold]}
    out["tools_list"] = {"bytes": tl_bytes, "tools": tl_count}

    c = Client(a.exe)
    c.init()
    out["surface"] = a.surface
    bed = None
    if a.surface == "hidden":
        env, err, dt = c.call("ghost_window", {"op": "launch", "exe": f'"{a.testbed}" --title "{TITLE}"'})
        out["launch"] = {"ms": round(dt, 1), "is_error": err, "resp": env}
    else:
        # The user's desktop, shown WITHOUT activation so the human's focus is untouched.
        si = subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        si.wShowWindow = 4  # SW_SHOWNOACTIVATE
        bed = subprocess.Popen([a.testbed, "--title", TITLE], startupinfo=si)
    time.sleep(1.0)

    def run(op, fn):
        for _ in range(2):  # warmup, not recorded
            fn(-1)
        rows = [fn(i) for i in range(a.n)]
        out["raw"][op] = rows
        out["ops"].append(summary(op, rows))

    def see_text(i):
        env, err, dt = c.call("ghost_see", {"mode": "text", "window": TITLE})
        txt = json.dumps(data_of(env))
        return {"ms": dt, "is_error": err, "correct": "Increment" in txt, "server_ms": env.get("ms")}

    def see_full(i):
        env, err, dt = c.call("ghost_see", {"window": TITLE})
        txt = json.dumps(data_of(env))
        return {"ms": dt, "is_error": err, "correct": "Increment" in txt and "Field" in txt,
                "server_ms": env.get("ms"), "bytes": len(txt)}

    def find(i):
        env, err, dt = c.call("ghost_find", {"name": "Increment", "role": "button", "window": TITLE})
        d = data_of(env)
        ok = (not err) and isinstance(d, dict) and bool(d.get("center"))
        return {"ms": dt, "is_error": err, "correct": ok, "server_ms": env.get("ms"),
                "source": d.get("source") if isinstance(d, dict) else None,
                "matches": d.get("matches") if isinstance(d, dict) else None}

    targets = {
        "increment": {"name": "Increment", "role": "button", "index": 0},
        "disabled": {"name": "Locked", "role": "button"},
        "label": {"name": "Field", "role": "text"},
        "caret-label": {"name": "Field", "role": "text"},
        "distant": {"name": "Note", "role": "button"},
    }

    def notes():
        env, _, _ = c.call("ghost_see", {"mode": "text", "window": TITLE})
        m = re.search(r"Status: noted (\d+)", json.dumps(data_of(env)))
        return int(m.group(1)) if m else 0

    def act_click(i):
        if a.control == "caret-label":
            c.call("ghost_act", {"action": "click", "role": "edit", "window": TITLE})
            time.sleep(0.1)
        t0 = window_title(c)
        n0 = notes() if a.control == "distant" else 0
        env, err, dt = c.call("ghost_act", {"action": a.action, "window": TITLE, **targets[a.control]})
        time.sleep(0.05)
        t1 = window_title(c)
        before, after = clicks(t0), clicks(t1)
        d = data_of(env)
        if a.control == "increment":
            ok = after == before + (2 if a.action == "double_click" else 1)
        elif a.control == "distant":
            ok = notes() == n0 + (2 if a.action == "double_click" else 1) and t1 == t0
        else:
            ok = t1 == t0
        return {"ms": dt, "is_error": err, "correct": ok, "before": before, "after": after,
                "verified": d.get("verified") if isinstance(d, dict) else None,
                "verified_by": d.get("verified_by") if isinstance(d, dict) else None,
                "error": json.dumps(env)[:300] if err else None,
                "server_ms": env.get("ms")}

    def act_type(i):
        tok = f"pz{i + 2}q"
        env, err, dt = c.call("ghost_act", {"action": "type", "role": "edit", "text_input": tok,
                                            "window": TITLE})
        time.sleep(0.05)
        renv, _, _ = c.call("ghost_see", {"mode": "text", "window": TITLE})
        d = data_of(env)
        return {"ms": dt, "is_error": err, "correct": tok in json.dumps(data_of(renv)),
                "verified": d.get("verified") if isinstance(d, dict) else None,
                "server_ms": env.get("ms")}

    if a.control == "increment":
        run("see_text", see_text)
        run("see_full", see_full)
        run("find", find)
    run(f"act_{a.action}[{a.control}]", act_click)
    if a.control == "increment":
        run("act_type", act_type)

    c.call("ghost_window", {"op": "state", "name": TITLE, "state": "close"})
    c.close()
    if bed is not None:
        try:
            bed.wait(timeout=5)
        except Exception:
            bed.terminate()

    with open(os.path.join(a.out, "latency.json"), "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, default=str)
    lines = [f"exe: {out['exe']}", f"surface: {a.surface}", f"control: {a.control}", "",
             f"cold start (spawn->initialize, n=10): median {out['cold_start']['median_ms']} ms, p95 {out['cold_start']['p95_ms']} ms",
             f"tools/list: {tl_count} tools, {tl_bytes} bytes", "",
             "| op | n | median ms | p95 ms | min | max | accuracy | errors | verified=true | verified=false | verified=null |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for s in out["ops"]:
        lines.append(f"| {s['op']} | {s['n']} | {s['median_ms']} | {s['p95_ms']} | {s['min_ms']} | {s['max_ms']} | "
                     f"{s['correct']}/{s['n']} | {s['errors']} | {s['verified_true']} | {s['verified_false']} | {s['verified_null']} |")
    md = "\n".join(lines) + "\n"
    with open(os.path.join(a.out, "latency.md"), "w", encoding="utf-8") as f:
        f.write(md)
    print(md)
    return 0 if all(s["accuracy"] == 1.0 for s in out["ops"]) else 1


if __name__ == "__main__":
    sys.exit(main())
