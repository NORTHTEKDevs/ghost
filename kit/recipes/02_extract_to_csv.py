#!/usr/bin/env python3
"""Recipe 2 - pull structured data OUT of an app that has no API.

THE PATTERN
    Point Ghost at a window, read every element the accessibility tree exposes,
    and write it to CSV. No export button, no database access, no vendor
    integration - just the same information a person can see.

WHY THIS EARNS ITS KEEP
    Recipe 1 puts data in; this gets it out. Between them they cover most of
    what "integrating" a legacy application actually means. Two immediate uses:

      1. Reverse-engineering. Run it against a screen you intend to automate and
         you get the exact role/name of every control - which is what you need to
         write a reliable selector instead of clicking at coordinates.
      2. Extraction. Pull a list, grid, or report out of software that has no
         export, on a schedule.

RUNNING IT
    python 02_extract_to_csv.py                       # demo: a Notepad window
    python 02_extract_to_csv.py "Notepad"             # any window, by partial title
    python 02_extract_to_csv.py "Notepad" out.csv     # choose the output file

ADAPTING IT
    Filter on role to grab just what you want - `dataitem` and `listitem` for
    grids and lists, `edit` and `document` for fields, `text` for labels. The
    --roles flag does this without editing the file.

Exit code 0 = at least one element was captured.
"""
import csv
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _ghost import Ghost, GhostError  # noqa: E402


def parse_args(argv):
    window, out, roles = "Notepad", "extracted.csv", None
    positional = []
    for a in argv:
        if a.startswith("--roles="):
            roles = {r.strip().lower() for r in a.split("=", 1)[1].split(",") if r.strip()}
        else:
            positional.append(a)
    if len(positional) >= 1:
        window = positional[0]
    if len(positional) >= 2:
        out = positional[1]
    return window, out, roles


def main():
    window, out_path, roles = parse_args(sys.argv[1:])
    print(f"reading window matching '{window}'")

    with Ghost() as g:
        # Launch the demo target only if we are using the default and it is absent.
        try:
            g.call("ghost_focus_window", name=window)
        except GhostError:
            if window != "Notepad":
                sys.exit(f"no visible window matching '{window}' - open it first")
            g.call("ghost_launch", exe="notepad.exe")
            time.sleep(2.0)
            g.call("ghost_focus_window", name=window)
        time.sleep(0.6)

        result = g.call("ghost_describe_screen", window=window)

    elements = result if isinstance(result, list) else (result or {}).get("elements", [])
    if not elements:
        sys.exit("no elements returned - is the window visible and not minimised?")

    if roles:
        elements = [e for e in elements if str(e.get("role", "")).lower() in roles]
        if not elements:
            sys.exit(f"no elements matched roles={sorted(roles)}")

    # describe_screen returns geometry + role + name. It does NOT return an
    # enabled flag - use ghost_snapshot if you need enabled/actionable.
    fields = ["role", "name", "left", "top", "right", "bottom"]
    with open(out_path, "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=fields, extrasaction="ignore")
        w.writeheader()
        for e in elements:
            w.writerow({k: e.get(k, "") for k in fields})

    by_role = {}
    for e in elements:
        by_role[e.get("role", "?")] = by_role.get(e.get("role", "?"), 0) + 1
    summary = ", ".join(f"{n} {r}" for r, n in sorted(by_role.items(), key=lambda kv: -kv[1])[:6])

    print(f"wrote {len(elements)} element(s) to {out_path}")
    print(f"  {summary}")
    print("\nUse the role+name pairs above as selectors, e.g.:")
    sample = next((e for e in elements if e.get("name")), None)
    if sample:
        print(f'  ghost_act action=click name="{sample["name"]}" role={sample.get("role", "button")}')
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except GhostError as e:
        sys.exit(f"ghost error: {e}")
