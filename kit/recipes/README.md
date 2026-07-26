# Recipes

Working starting points for the two jobs that make up most real automation
work: getting data **into** an application that has no API, and getting it
**out** again.

Each recipe runs as-is against Notepad so you can watch it work before trusting
it with anything that matters, and each one is written to be adapted — the app,
the window, and the field are constants at the top of the file.

| Recipe | What it does |
| --- | --- |
| `01_batch_data_entry.py` | Reads a CSV, types each row into an app, **reads it back**, and halts on the first row that does not land. |
| `02_extract_to_csv.py` | Reads every element a window exposes and writes it to CSV — for extraction, and for finding reliable selectors. |

```bash
python 01_batch_data_entry.py                 # demo
python 01_batch_data_entry.py invoices.csv    # your data

python 02_extract_to_csv.py                   # demo
python 02_extract_to_csv.py "Your App" out.csv
python 02_extract_to_csv.py "Your App" out.csv --roles=dataitem,listitem
```

Requires Python 3.8+. Nothing to install — `_ghost.py` speaks to `ghost-mcp.exe`
directly and finds it next to the kit or on PATH.

---

## The idea worth stealing

Look at the loop in recipe 1. After every write it reads the value back, and if
the readback does not contain what it sent, **it stops**.

That is the difference between automation you can leave running and automation
you have to babysit. The common failure in this kind of work is not a crash —
it's row 40 of 200 silently not landing, and nobody noticing until the numbers
are wrong at month end. A run that halts on row 40 and tells you so costs you
ten minutes. A run that doesn't costs you a reconciliation.

`_ghost.py` applies the same principle to itself. Ghost reports failures two
ways — as a JSON-RPC error, and as a payload with `ok: false` — and the client
raises on **both**. An earlier version of it checked only the first, and the
result was a recipe that cheerfully printed "verified" for rows that were never
typed. If you write your own client, check both.

## Picking a target

Run recipe 2 against the screen you want to automate first. The CSV it produces
is the list of selectors available to you:

```
role,name,left,top,right,bottom
document,Text editor,20,127,1914,890
button,Add New Tab,313,42,345,66
```

Prefer `name` + `role` over coordinates — names survive a window move or a
resolution change and coordinates do not. If an element has no usable name,
`ghost_find` also supports a natural-language description via the optional
vision fallback.

## When a recipe cannot find your control

1. Run `ghost doctor`. A FAIL there explains most problems outright.
2. Run recipe 2 against the window and look at what roles actually exist. Win11
   apps built on WinUI often expose a text area as `document` rather than
   `edit`, for instance.
3. If the app draws its own UI (a canvas, a game engine, some CAD and terminal
   apps), the accessibility tree will be nearly empty. That is what the vision
   fallback is for — set a vision key and use `ghost_find` with a description.

Still stuck? Reply to your receipt email with the `ghost doctor` output and the
recipe-2 CSV for the window. That is usually everything needed to answer it.
