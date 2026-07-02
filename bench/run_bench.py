#!/usr/bin/env python3
"""
Ghost MCP benchmark harness.

Drives the compiled `ghost-mcp` binary over stdio JSON-RPC (the exact transport a
real MCP client uses) through a suite of real Windows desktop tasks, and scores
each task by reading an ACTUAL post-condition back from the screen -- never by
trusting that a tool call "returned ok". A task passes only if the observable
world changed the way it should (e.g. the Calculator display really reads 42).

Every task is self-contained: it launches its own app, verifies, and cleans up,
so the suite is safe to run on any Windows 10/11 machine and reproducible by
anyone. Results (pass/fail, latency, grounding tier) are written to
results/latest.json plus a human-readable results/latest.md.

Usage:
    python bench/run_bench.py [--exe PATH_TO_ghost-mcp.exe] [--json OUT] [--md OUT]

Exit code 0 iff every task passed (so CI can gate on it).
"""
import argparse
import base64
import json
import os
import re
import statistics
import subprocess
import sys
import time

DEFAULT_EXE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "target", "release", "ghost-mcp.exe",
)


class GhostClient:
    """Minimal stdio JSON-RPC client for ghost-mcp -- one subprocess, serial calls."""

    def __init__(self, exe):
        self.p = subprocess.Popen(
            [exe],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        self._id = 0
        self._send("initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "ghost-bench", "version": "1"},
        })
        self._read()

    def _send(self, method, params=None):
        self._id += 1
        req = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            req["params"] = params
        self.p.stdin.write(json.dumps(req) + "\n")
        self.p.stdin.flush()

    def _read(self):
        return json.loads(self.p.stdout.readline())

    def call(self, name, args):
        """Invoke a tool. Returns (envelope_dict, is_error, latency_ms)."""
        t0 = time.perf_counter()
        self._send("tools/call", {"name": name, "arguments": args})
        resp = self._read()
        dt = (time.perf_counter() - t0) * 1000.0
        res = resp.get("result", {})
        is_error = bool(res.get("isError", False))
        env = {}
        try:
            env = json.loads(res["content"][0]["text"]) or {}
        except Exception:
            env = {}
        return env, is_error, dt

    def data(self, name, args):
        env, err, dt = self.call(name, args)
        return (env.get("data") if isinstance(env.get("data"), (dict, list, str)) else env), err, dt

    def close(self):
        try:
            self.p.stdin.close()
        except Exception:
            pass
        self.p.terminate()


def _sleep(ms):
    time.sleep(ms / 1000.0)


# --------------------------------------------------------------------------- #
# Task helpers                                                                 #
# --------------------------------------------------------------------------- #

def _launch_calc(g):
    g.call("ghost_window", {"op": "launch", "exe": "calc.exe"})
    _sleep(1500)
    g.call("ghost_wait", {"for": "idle", "window": "Calculator", "timeout_ms": 4000})
    g.call("ghost_window", {"op": "focus", "name": "Calculator"})
    _sleep(300)


def _close_calc(g):
    g.call("ghost_window", {"op": "state", "name": "Calculator", "state": "close"})
    _sleep(300)


def _calc_display_value(g):
    """Read the Calculator result as an exact numeric string. The display is a
    'text' element named like 'Display is 42'; we extract exactly the value after
    'Display is ' so comparison is exact (not a substring match where '42' would
    also match '420'). Returns (value_str_or_None, raw_line_for_detail)."""
    env, _, _ = g.data("ghost_see", {"mode": "text", "window": "Calculator"})
    text = env.get("text", "") if isinstance(env, dict) else ""
    for line in text.splitlines():
        m = re.match(r"^Display is\s+(.+?)\s*$", line)
        if m:
            return m.group(1).strip(), line.strip()
    return None, (text[:60] if text else "(no display text)")


def _looks_like_image(b64, min_bytes=200):
    """True if the base64 payload decodes to a real PNG or JPEG of plausible size
    — proves an actual capture, not a stub returning a byte count."""
    try:
        raw = base64.b64decode(b64)
    except Exception:
        return False
    if len(raw) < min_bytes:
        return False
    return raw[:3] == b"\xff\xd8\xff" or raw[:8] == b"\x89PNG\r\n\x1a\n"


# --------------------------------------------------------------------------- #
# Tasks. Each returns (passed: bool, detail: str, source: str|None).          #
# A task must verify a real post-condition, not just that a call returned ok.  #
# --------------------------------------------------------------------------- #

def task_find_button(g):
    """Perception+grounding: locate a named button, get real coordinates."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_find", {"name": "Seven", "role": "button", "window": "Calculator"})
        d = env.get("data") or {}
        c = d.get("center") or {}
        ok = (not err) and d.get("has_rect") and isinstance(c.get("x"), int) and c.get("x") > 0
        return ok, f"center={c} source={d.get('source')}", d.get("source")
    finally:
        _close_calc(g)


def task_click_compute(g):
    """Action+verify: click 6 x 7 = and confirm the display EXACTLY reads 42
    (independent read-back, exact match)."""
    _launch_calc(g)
    try:
        for b in ["Six", "Multiply by", "Seven", "Equals"]:
            g.call("ghost_act", {"action": "click", "name": b, "role": "button", "window": "Calculator"})
            _sleep(150)
        val, line = _calc_display_value(g)
        ok = val == "42"
        return ok, f"display={line!r}", "uia"
    finally:
        _close_calc(g)


def task_keyboard_compute(g):
    """Keyboard input: type '9*9=' and confirm the display EXACTLY reads 81.
    Exercises symbol-key entry ('*') on the keyboard path."""
    _launch_calc(g)
    try:
        for k in ["9", "*", "9", "Enter"]:
            g.call("ghost_key", {"keys": k, "window": "Calculator"})
            _sleep(150)
        val, line = _calc_display_value(g)
        ok = val == "81"
        return ok, f"display={line!r}", "keyboard"
    finally:
        _close_calc(g)


def task_act_verified_flag(g):
    """Verification honesty: a real click reports verified=true (screen changed)."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_act", {"action": "click", "name": "Five", "role": "button", "window": "Calculator"})
        d = env.get("data") or {}
        ok = (not err) and d.get("verified") is True and d.get("focus_confirmed") is True
        return ok, f"verified={d.get('verified')} focus={d.get('focus_confirmed')}", d.get("source")
    finally:
        _close_calc(g)


def task_wait_for_element(g):
    """Wait primitive: after launch, wait for the Equals button to appear, then
    INDEPENDENTLY confirm it really exists via ghost_find (don't trust wait's own
    ok/appeared flags)."""
    g.call("ghost_window", {"op": "launch", "exe": "calc.exe"})
    try:
        env, err, _ = g.call("ghost_wait", {"for": "element", "role": "button", "name": "Equals", "timeout_ms": 5000})
        d = env.get("data") or {}
        waited_ok = (not err) and d.get("ok") is True and d.get("appeared") is True
        # Independent re-observation:
        fenv, ferr, _ = g.call("ghost_find", {"name": "Equals", "role": "button", "window": "Calculator", "mode": "instant_only"})
        really_there = (not ferr) and (fenv.get("data") or {}).get("has_rect") is True
        ok = waited_ok and really_there
        return ok, f"appeared={d.get('appeared')} confirmed={really_there}", None
    finally:
        _close_calc(g)


def task_window_list_state(g):
    """Window management: the just-focused Calculator appears in the list as
    'normal' (not minimized)."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_window", {"op": "list"})
        wins = (env.get("data") or {}).get("windows", [])
        match = [w for w in wins if "calculator" in w.get("name", "").lower()]
        ok = (not err) and len(match) == 1 and match[0].get("state") == "normal"
        return ok, f"found={len(match)} state={match[0].get('state') if match else None}", None
    finally:
        _close_calc(g)


def task_read_text(g):
    """Text extraction: type a known value, then read it back from the a11y tree
    (dynamic content, not just static labels)."""
    _launch_calc(g)
    try:
        for k in ["1", "2", "3"]:
            g.call("ghost_key", {"keys": k, "window": "Calculator"})
            _sleep(120)
        val, line = _calc_display_value(g)
        ok = val == "123"
        return ok, f"read_back={line!r}", None
    finally:
        _close_calc(g)


def task_index_disambiguation(g):
    """Disambiguation: several buttons exist; index=0 returns a match + count."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_find", {"role": "button", "index": 0, "window": "Calculator"})
        d = env.get("data") or {}
        ok = (not err) and isinstance(d.get("matches"), int) and d.get("matches") > 1 and d.get("has_rect")
        return ok, f"matches={d.get('matches')} name={d.get('name')!r}", d.get("source")
    finally:
        _close_calc(g)


def task_run_chaining(g):
    """Flow chaining: compute 8+5 by finding each button in one step and clicking
    its chained ${steps.N.center} in the next, then INDEPENDENTLY verify the
    display reads 13 — proving the substituted coordinates were actually correct,
    not just that the flow reported completed."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_run", {"steps": [
            {"op": "ghost_find", "name": "Eight", "role": "button", "window": "Calculator"},
            {"op": "ghost_click_at", "x": "${steps.0.center.x}", "y": "${steps.0.center.y}"},
            {"op": "ghost_find", "name": "Plus", "role": "button", "window": "Calculator"},
            {"op": "ghost_click_at", "x": "${steps.2.center.x}", "y": "${steps.2.center.y}"},
            {"op": "ghost_find", "name": "Five", "role": "button", "window": "Calculator"},
            {"op": "ghost_click_at", "x": "${steps.4.center.x}", "y": "${steps.4.center.y}"},
            {"op": "ghost_find", "name": "Equals", "role": "button", "window": "Calculator"},
            {"op": "ghost_click_at", "x": "${steps.6.center.x}", "y": "${steps.6.center.y}"},
        ]})
        d = env.get("data") or {}
        flow_ok = (not err) and d.get("ok") is True and d.get("failed") == 0
        val, line = _calc_display_value(g)
        ok = flow_ok and val == "13"
        return ok, f"completed={d.get('completed')}/{d.get('total')} display={line!r}", None
    finally:
        _close_calc(g)


def task_structured_error(g):
    """Structured errors: a missing element yields error_code + suggested_action."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_find", {"name": "NoSuchElementZZZ", "mode": "instant_only", "window": "Calculator"})
        ok = err and env.get("error_code") == -32001 and isinstance(env.get("suggested_action"), str)
        return ok, f"code={env.get('error_code')} has_suggestion={'suggested_action' in env}", None
    finally:
        _close_calc(g)


def task_screenshot_element(g):
    """Element screenshot: crop to one button and confirm the payload is a REAL
    decodable JPEG/PNG (magic bytes + plausible size), not just a positive byte
    count."""
    _launch_calc(g)
    try:
        env, err, _ = g.call("ghost_screenshot", {"name": "Equals", "role": "button"})
        d = env.get("data") or {}
        b64 = d.get("jpeg_base64") or d.get("png_base64") or ""
        n = d.get("size_bytes", 0)
        ok = (not err) and bool(b64) and _looks_like_image(b64)
        return ok, f"size_bytes={n} valid_image={_looks_like_image(b64) if b64 else False}", None
    finally:
        _close_calc(g)


def task_value_equals_assert(g):
    """Assertion: value-equals reads a real element value (uses a fresh Notepad tab
    so it never touches the user's open documents)."""
    g.call("ghost_window", {"op": "launch", "exe": "notepad.exe"})
    _sleep(1500)
    g.call("ghost_wait", {"for": "idle", "window": "Notepad", "timeout_ms": 4000})
    g.call("ghost_key", {"keys": "Ctrl+N", "window": "Notepad"})   # fresh tab
    _sleep(600)
    try:
        g.call("ghost_act", {"action": "type", "name": "Text editor", "role": "document",
                             "text_input": "benchmark-value"})
        # Independent re-read of the field value via get_text (not the assert tool
        # that could self-report), THEN the assert as a second signal.
        renv, _, _ = g.data("ghost_get_text", {"role": "document"})
        actual = renv.get("text", "") if isinstance(renv, dict) else (renv or "")
        env, err, _ = g.call("ghost_assert", {"predicate": "value-contains", "name": "Text editor",
                                              "role": "document", "text": "benchmark-value"})
        d = env.get("data") or {}
        ok = (actual.strip() == "benchmark-value") and (not err) and d.get("passed") is True
        return ok, f"read_back={actual.strip()!r} assert_passed={d.get('passed')}", "uia"
    finally:
        # Discard the fresh tab; never save. All cleanup is scoped to Notepad so a
        # stray dialog in another app can never be clicked.
        g.call("ghost_key", {"keys": "Ctrl+A", "window": "Notepad"})
        g.call("ghost_key", {"keys": "Delete", "window": "Notepad"})
        g.call("ghost_key", {"keys": "Ctrl+W", "window": "Notepad"})
        _sleep(500)
        env, _, _ = g.call("ghost_see", {"mode": "fast", "window": "Notepad"})
        els = (env.get("data") or {}).get("elements", [])
        if any(e.get("name") == "Don't save" for e in els):
            g.call("ghost_act", {"action": "click", "name": "Don't save", "role": "button", "window": "Notepad"})
        _sleep(300)


TASKS = [
    ("find_button", "Locate a named button and return real coordinates", task_find_button),
    ("click_compute", "Click 6 x 7 = and verify display reads 42", task_click_compute),
    ("keyboard_compute", "Type 9*9= and verify display reads 81", task_keyboard_compute),
    ("act_verified", "Action reports honest verified/focus_confirmed", task_act_verified_flag),
    ("wait_for_element", "Wait for a button to appear after launch", task_wait_for_element),
    ("window_list_state", "Window appears in list with a state field", task_window_list_state),
    ("read_text", "Read UI element labels from the a11y tree", task_read_text),
    ("index_disambiguation", "Select nth match with a matches count", task_index_disambiguation),
    ("run_chaining", "ghost_run step output feeds the next step", task_run_chaining),
    ("structured_error", "Missing element yields code + suggested_action", task_structured_error),
    ("screenshot_element", "Crop a screenshot to one element", task_screenshot_element),
    ("value_equals_assert", "Assert an element's real value after typing", task_value_equals_assert),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=DEFAULT_EXE)
    ap.add_argument("--json", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "results", "latest.json"))
    ap.add_argument("--md", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "results", "latest.md"))
    ap.add_argument("--stamp", default="", help="ISO timestamp to record (host passes real clock)")
    args = ap.parse_args()

    exe = os.path.abspath(args.exe)
    if not os.path.exists(exe):
        print(f"ERROR: ghost-mcp binary not found at {exe}\nBuild it: cargo build --release -p ghost-mcp", file=sys.stderr)
        return 2

    g = GhostClient(exe)
    results = []
    print(f"Running {len(TASKS)} tasks against {exe}\n")
    for tid, desc, fn in TASKS:
        t0 = time.perf_counter()
        try:
            passed, detail, source = fn(g)
        except Exception as e:  # a crash in a task is a FAIL, not a harness abort
            passed, detail, source = False, f"exception: {e}", None
        dt = (time.perf_counter() - t0) * 1000.0
        results.append({
            "id": tid, "description": desc, "passed": bool(passed),
            "detail": detail, "grounding_source": source, "latency_ms": round(dt, 1),
        })
        mark = "PASS" if passed else "FAIL"
        print(f"  [{mark}] {tid:22} {dt:8.0f} ms  {detail}")
    g.close()

    passed_n = sum(1 for r in results if r["passed"])
    total = len(results)
    lat = [r["latency_ms"] for r in results]
    summary = {
        "tasks": total,
        "passed": passed_n,
        "failed": total - passed_n,
        "success_rate": round(100.0 * passed_n / total, 1) if total else 0.0,
        "latency_ms_median": round(statistics.median(lat), 1) if lat else 0.0,
        "latency_ms_max": round(max(lat), 1) if lat else 0.0,
        "timestamp": args.stamp,
        "binary": exe,
    }
    out = {"summary": summary, "results": results}

    os.makedirs(os.path.dirname(args.json), exist_ok=True)
    with open(args.json, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=2)
    _write_md(args.md, out)

    print(f"\n{passed_n}/{total} passed ({summary['success_rate']}%)  "
          f"median {summary['latency_ms_median']} ms, max {summary['latency_ms_max']} ms")
    print(f"Wrote {args.json} and {args.md}")
    return 0 if passed_n == total else 1


def _write_md(path, out):
    s = out["summary"]
    lines = [
        "# Ghost MCP benchmark results",
        "",
        f"- Tasks: **{s['passed']}/{s['tasks']} passed ({s['success_rate']}%)**",
        f"- Latency: median {s['latency_ms_median']} ms, max {s['latency_ms_max']} ms (full task incl. app launch)",
    ]
    if s.get("timestamp"):
        lines.append(f"- Run: {s['timestamp']}")
    lines += ["", "| Task | Result | Latency | Detail |", "|---|---|---|---|"]
    for r in out["results"]:
        mark = "PASS" if r["passed"] else "FAIL"
        detail = str(r["detail"]).replace("|", "\\|")
        lines.append(f"| {r['id']} | {mark} | {r['latency_ms']:.0f} ms | {detail} |")
    lines.append("")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


if __name__ == "__main__":
    sys.exit(main())
