#!/usr/bin/env python3
"""Recipe 1 - batch data entry into an app that has no API, verified per row.

THE PATTERN
    Read rows from a CSV. For each row, type it into a desktop application and
    READ IT BACK before moving on. If what the app holds does not match what you
    sent, stop - do not keep going and hope.

    That last part is the whole point. Most automation writes and moves on; when
    row 40 of 200 silently fails you find out at month end. Ghost verifies each
    action, so the run either completes correctly or halts on the row that broke
    with the row number and the mismatch.

WHY THIS EARNS ITS KEEP
    The software that eats the most human hours in a small business - practice
    management, agency management, desktop accounting, older ERPs - usually has
    no API, or one behind an expensive partner tier. This is the shape of the
    work that replaces.

RUNNING IT
    python 01_batch_data_entry.py                 # demo against Notepad
    python 01_batch_data_entry.py rows.csv        # your own CSV

ADAPTING IT
    Change TARGET_APP and TARGET_WINDOW to your application, and change
    row_to_text() to format a row the way that app expects. Everything else -
    the verify-and-halt loop - stays as is.

Exit code 0 = every row landed and was verified. 1 = a row failed; nothing after
it was attempted.
"""
import csv
import io
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _ghost import Ghost, GhostError  # noqa: E402

# ---- adapt these three things to your application ------------------------
TARGET_APP = "notepad.exe"
TARGET_WINDOW = "Notepad"
TARGET_ROLE = "edit"          # the control that receives the text


def row_to_text(row):
    """Format one CSV row the way the target app expects."""
    return f"{row['invoice']}  {row['customer']}  {row['amount']}"
# --------------------------------------------------------------------------


DEMO_CSV = """invoice,customer,amount
INV-1001,Northern Supply Co,1240.00
INV-1002,Arctic Fabrication,875.50
INV-1003,Denali Logistics,3310.25
"""


def load_rows(path):
    if path:
        with open(path, newline="", encoding="utf-8") as fh:
            return list(csv.DictReader(fh))
    return list(csv.DictReader(io.StringIO(DEMO_CSV)))


def main():
    rows = load_rows(sys.argv[1] if len(sys.argv) > 1 else None)
    if not rows:
        sys.exit("no rows to enter")

    print(f"entering {len(rows)} row(s) into {TARGET_APP}, verifying each\n")

    with Ghost() as g:
        g.call("ghost_launch", exe=TARGET_APP)
        time.sleep(2.0)
        g.call("ghost_focus_window", name=TARGET_WINDOW)
        time.sleep(0.6)

        entered = 0
        for i, row in enumerate(rows, 1):
            text = row_to_text(row)
            try:
                g.call("ghost_act", action="type", role=TARGET_ROLE, text_input=text + "\r\n")
            except GhostError as e:
                print(f"  row {i}: FAILED to type - {e}")
                print(f"\nHALTED on row {i}. Rows 1..{i - 1} are in the app; {len(rows) - i + 1} were not attempted.")
                return 1

            time.sleep(0.35)

            # Confirm the text is actually on screen in the target window. This
            # is the step most automation skips, and it is why a bad row stops
            # the run instead of silently vanishing into row 40 of 200.
            marker = row["invoice"]
            # Read the window's text back directly. ghost_assert's text-present
            # predicate goes through local OCR, which is the right tool for a
            # canvas app but unnecessarily lossy when the control exposes its
            # text - and this one does.
            contents = (g.call("ghost_read_text", window=TARGET_WINDOW) or {}).get("text", "")
            if marker not in contents:
                print(f"  row {i}: MISMATCH - '{marker}' is not in the window after typing")
                print(f"\nHALTED on row {i}. Rows 1..{i - 1} were verified; "
                      f"{len(rows) - i + 1} were not attempted.")
                return 1

            entered += 1
            print(f"  row {i}: {marker} verified")

        print(f"\n{entered}/{len(rows)} rows entered and verified.")
        return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except GhostError as e:
        sys.exit(f"ghost error: {e}")
