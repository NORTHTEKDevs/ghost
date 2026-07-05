#!/usr/bin/env python3
"""Ghost reliability soak test.

Drives the real ghost-mcp binary through many act-then-verify cycles on
Calculator and measures the reliability signals that unit tests can't: how often
`verified` comes back null/false, how often focus is lost, action latency
percentiles, and whether the action's real effect actually happened (the display
reads the expected sum). Exits 0 iff every threshold holds.

This is the harness that would have caught the v0.10.0 static-screen regression
(region captures stopped warming the cache -> verify silently went null): that
bug shows up here as a verify-null-rate spike.

Usage:
  python bench/soak.py                 # default 40 cycles (~160 acts)
  python bench/soak.py --cycles 250    # ~1000 acts
  python bench/soak.py --self-test     # assert the harness catches a planted wrong effect
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from run_bench import GhostClient, _launch_calc, _close_calc, _calc_display_value, DEFAULT_EXE, _sleep

DIGIT = {1: "One", 2: "Two", 3: "Three", 4: "Four", 5: "Five",
         6: "Six", 7: "Seven", 8: "Eight", 9: "Nine"}


def _pct(sorted_vals, p):
    if not sorted_vals:
        return 0.0
    k = min(len(sorted_vals) - 1, int(round((p / 100.0) * (len(sorted_vals) - 1))))
    return sorted_vals[k]


def run_soak(cycles, exe, plant_wrong=False):
    g = GhostClient(exe)
    acts = []          # per-act reliability records
    cycle_results = [] # per-cycle effect correctness
    try:
        _launch_calc(g)
        for i in range(cycles):
            a = 1 + (i % 9)
            b = 1 + ((i * 7) % 9)
            expected = str(a + b)
            if plant_wrong:
                expected = str(a + b + 1)  # deliberately wrong -> effect must FAIL

            # Reset via keyboard Escape (setup, not measured).
            g.call("ghost_key", {"keys": "Escape", "window": "Calculator"})
            _sleep(40)

            seq = [DIGIT[a], "Plus", DIGIT[b], "Equals"]
            for name in seq:
                env, err, dt = g.call(
                    "ghost_act",
                    {"action": "click", "name": name, "role": "button", "window": "Calculator"},
                )
                data = env.get("data", env) if isinstance(env, dict) else {}
                acts.append({
                    "cycle": i, "button": name, "is_error": bool(err),
                    "verified": data.get("verified"),
                    "focus_confirmed": data.get("focus_confirmed"),
                    "warning": data.get("warning"),
                    "ms": dt,
                })

            val, _ = _calc_display_value(g)
            cycle_results.append({"cycle": i, "expected": expected, "got": val,
                                  "ok": (val == expected)})
        _close_calc(g)
    finally:
        g.close()

    n = len(acts)
    lat = sorted(a["ms"] for a in acts)
    verify_null = sum(1 for a in acts if a["verified"] is None)
    verify_false = sum(1 for a in acts if a["verified"] is False)
    focus_lost = sum(1 for a in acts if a["focus_confirmed"] is False)
    warnings = sum(1 for a in acts if a["warning"])
    errors = sum(1 for a in acts if a["is_error"])
    effect_ok = sum(1 for c in cycle_results if c["ok"])
    effect_mismatch = len(cycle_results) - effect_ok

    return {
        "cycles": cycles,
        "acts": n,
        "verify_null_rate": round(verify_null / n, 4) if n else 1.0,
        "verify_false_rate": round(verify_false / n, 4) if n else 1.0,
        "focus_lost_rate": round(focus_lost / n, 4) if n else 1.0,
        "warning_rate": round(warnings / n, 4) if n else 1.0,
        "error_rate": round(errors / n, 4) if n else 1.0,
        "effect_mismatch": effect_mismatch,
        "effect_mismatch_rate": round(effect_mismatch / len(cycle_results), 4) if cycle_results else 1.0,
        "latency_ms": {
            "p50": round(_pct(lat, 50), 1),
            "p95": round(_pct(lat, 95), 1),
            "p99": round(_pct(lat, 99), 1),
            "max": round(lat[-1], 1) if lat else 0.0,
        },
    }


# Fail thresholds. Chosen so the v0.10.0 verify-null regression (which spiked
# verify_null_rate) and any real effect breakage trip the gate.
THRESHOLDS = {
    "verify_null_rate": 0.10,
    "verify_false_rate": 0.15,
    "focus_lost_rate": 0.10,
    "error_rate": 0.02,
    "effect_mismatch_rate": 0.0,
}


def judge(stats):
    fails = []
    for k, limit in THRESHOLDS.items():
        if stats[k] > limit:
            fails.append(f"{k}={stats[k]} > {limit}")
    return fails


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycles", type=int, default=40)
    ap.add_argument("--exe", default=DEFAULT_EXE)
    ap.add_argument("--self-test", action="store_true",
                    help="plant a wrong expected effect; harness PASSES iff it flags the mismatch")
    ap.add_argument("--out", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "results", "soak"))
    args = ap.parse_args()

    if args.self_test:
        stats = run_soak(max(3, args.cycles // 8), args.exe, plant_wrong=True)
        caught = stats["effect_mismatch"] == stats["cycles"]  # every cycle must be flagged wrong
        print(json.dumps(stats, indent=2))
        print(f"SELF-TEST: planted wrong effect on all {stats['cycles']} cycles; "
              f"harness flagged {stats['effect_mismatch']} -> {'PASS' if caught else 'FAIL'}")
        sys.exit(0 if caught else 1)

    t0 = time.time()
    stats = run_soak(args.cycles, args.exe)
    stats["wall_s"] = round(time.time() - t0, 1)
    fails = judge(stats)
    stats["verdict"] = "PASS" if not fails else "FAIL"
    stats["failures"] = fails

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out + ".json", "w", encoding="utf-8") as f:
        json.dump(stats, f, indent=2)
    _write_md(args.out + ".md", stats)

    print(json.dumps(stats, indent=2))
    print("VERDICT:", stats["verdict"])
    sys.exit(0 if not fails else 1)


def _write_md(path, s):
    lat = s["latency_ms"]
    lines = [
        "# Ghost reliability soak",
        "",
        f"**{s['verdict']}** - {s['acts']} acts over {s['cycles']} cycles "
        f"({s.get('wall_s', '?')}s wall).",
        "",
        "| Metric | Value | Threshold |",
        "| --- | --- | --- |",
        f"| verify-null rate | {s['verify_null_rate']} | <= {THRESHOLDS['verify_null_rate']} |",
        f"| verify-false rate | {s['verify_false_rate']} | <= {THRESHOLDS['verify_false_rate']} |",
        f"| focus-lost rate | {s['focus_lost_rate']} | <= {THRESHOLDS['focus_lost_rate']} |",
        f"| error rate | {s['error_rate']} | <= {THRESHOLDS['error_rate']} |",
        f"| effect-mismatch rate | {s['effect_mismatch_rate']} | == {THRESHOLDS['effect_mismatch_rate']} |",
        f"| latency p50 / p95 / p99 / max (ms) | {lat['p50']} / {lat['p95']} / {lat['p99']} / {lat['max']} | - |",
        "",
        "Effect correctness is re-observed (the Calculator display must read the "
        "expected sum), never trusted from the tool's return. `--self-test` plants "
        "a wrong expected value and passes only if the harness flags every cycle.",
    ]
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
