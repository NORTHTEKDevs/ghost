#!/usr/bin/env python3
"""Run the cross-tool comparison and write a matrix.

Today only the Ghost adapter is implemented, so it's the only column that runs;
competitor columns show as "-" (no adapter). This is deliberate and honest: the
harness MEASURES whoever has an adapter rather than asserting competitor numbers.
See README.md to plug a competitor in.

    python bench/compare/run.py            # runs Ghost over the task set
    python bench/compare/run.py --json     # machine-readable
"""
import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tasks import TASKS  # noqa: E402
from adapters import GhostAdapter  # noqa: E402

# Register the adapters that are actually runnable on this machine.
ADAPTERS = [GhostAdapter()]
# Competitor columns are declared here so the matrix header is honest about scope;
# add an adapter instance above to make a column run.
DECLARED_COLUMNS = ["Playwright-MCP", "Computer Use", "cua-driver"]

OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    results = {}  # tool -> task_id -> Result
    for ad in ADAPTERS:
        results[ad.name] = {}
        for task in TASKS:
            r = ad.run(task["id"])
            results[ad.name][task["id"]] = r
            mark = "PASS" if r.passed else ("N/A" if not r.applicable else "FAIL")
            print(f"[{ad.name}] {task['id']:<28} {mark:<5} {r.ms:6.0f}ms  {r.detail}")

    os.makedirs(OUT_DIR, exist_ok=True)
    _write_md(os.path.join(OUT_DIR, "compare.md"), results)
    if args.json:
        payload = {tool: {tid: vars(r) for tid, r in tr.items()} for tool, tr in results.items()}
        with open(os.path.join(OUT_DIR, "compare.json"), "w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2)

    ran = ADAPTERS[0].name
    passed = sum(1 for r in results[ran].values() if r.passed)
    total = len(TASKS)
    print(f"\n{ran}: {passed}/{total} tasks passed.")
    # Exit non-zero if the runnable adapter regressed, so this is also a self-test.
    sys.exit(0 if passed == total else 1)


def _cell(r):
    if r is None:
        return "-"
    if not r.applicable:
        return "N/A"
    return "PASS" if r.passed else "FAIL"


def _write_md(path, results):
    tool_cols = [ad.name for ad in ADAPTERS] + DECLARED_COLUMNS
    lines = [
        "# Cross-tool comparison (measured)",
        "",
        "Only tools with an implemented adapter are RUN here. Columns without an "
        "adapter show `-` (not measured) — this harness never asserts competitor "
        "numbers it didn't run. To add a tool, see [README.md](../README.md).",
        "",
        "| Task | " + " | ".join(tool_cols) + " |",
        "| --- | " + " | ".join("---" for _ in tool_cols) + " |",
    ]
    for task in TASKS:
        row = [f"{task['title']}"]
        for ad in ADAPTERS:
            row.append(_cell(results.get(ad.name, {}).get(task["id"])))
        for _ in DECLARED_COLUMNS:
            row.append("-")
        lines.append("| " + " | ".join(row) + " |")
    lines += [
        "",
        "`-` = no adapter run (not measured). `N/A` = tool can't attempt the task "
        "(e.g. a browser-only tool on a native-app task).",
    ]
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
