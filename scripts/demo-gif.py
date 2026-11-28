"""Builds assets/demo.gif from the output of scripts/demo-capture.mjs.

    python scripts/demo-gif.py <capture-dir> assets/demo.gif
    python scripts/demo-gif.py --stacked --result-first <capture-dir> assets/demo-stacked.gif

--stacked lays each frame out in one narrow column with large type (the
screenshot, then the call and what Ghost returned), so it still reads when a
phone shows it about 360 px wide. --result-first opens on the last step
(the verified result and the foreground audit), then plays the run from the
start, so the first thing a reader sees is the payoff.

Each frame shows the tool call an agent made, the fields Ghost returned, and
the screenshot Ghost took of its own window on the hidden desktop. Nothing is
drawn that the capture did not return. Needs Pillow.
"""
import json
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 960, 420
BG, PANEL, BORDER = "#0d1117", "#161b22", "#30363d"
TEXT, MUTED, BLUE, GREEN, AMBER = "#e6edf3", "#8b949e", "#58a6ff", "#3fb950", "#d29922"


def font(names, size):
    for name in names:
        for base in ("C:/Windows/Fonts/", "/usr/share/fonts/truetype/dejavu/", ""):
            try:
                return ImageFont.truetype(base + name, size)
            except OSError:
                continue
    return ImageFont.load_default()


MONO = font(["consola.ttf", "DejaVuSansMono.ttf"], 15)
MONO_B = font(["consolab.ttf", "DejaVuSansMono-Bold.ttf"], 15)
SANS = font(["segoeui.ttf", "DejaVuSans.ttf"], 14)
HEAD = font(["seguisb.ttf", "DejaVuSans-Bold.ttf"], 21)


def fmt(value):
    return json.dumps(value) if isinstance(value, str) else str(value).lower()


def response_lines(name, r):
    if name == "ghost_window":
        return [("surface", r["surface"]), ("desktop", r["desktop"]), ("anchored", r["target"]["title"])]
    lines = [("verified", r["verified"])]
    if r.get("verified_by"):
        lines.append(("verified_by", r["verified_by"]))
    lines += [("focus_preserved", r["focus_preserved"]), ("cursor_preserved", r["cursor_preserved"])]
    return lines


def frame(step, index, total, shot, audit=None):
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    d.text((32, 22), step["label"], font=HEAD, fill=TEXT)
    d.text((W - 32, 28), f"{index + 1} / {total}", font=SANS, fill=MUTED, anchor="ra")

    sx, sy = 408, 72
    lines = len(step["call"]["args"]) + len(response_lines(step["call"]["name"], step["response"]))
    bottom = max(sy + shot.height, 88 + 24 + 22 + 14 + 24 + 20 * lines + 12)
    d.rounded_rectangle((32, 72, 392, bottom), radius=8, fill=PANEL, outline=BORDER)
    x, y = 50, 88
    d.text((x, y), "agent calls", font=SANS, fill=MUTED)
    y += 24
    d.text((x, y), step["call"]["name"], font=MONO_B, fill=BLUE)
    y += 22
    for key, value in step["call"]["args"].items():
        if key == "exe":
            value = "ghost-testbed.exe ..."
        d.text((x + 14, y), f"{key}: {fmt(value)}", font=MONO, fill=TEXT)
        y += 20
    y += 14
    d.text((x, y), "Ghost returns", font=SANS, fill=MUTED)
    y += 24
    for key, value in response_lines(step["call"]["name"], step["response"]):
        color = GREEN if value is True or value == "hidden" else TEXT
        d.text((x + 14, y), f"{key}: {fmt(value)}", font=MONO, fill=color)
        y += 20

    d.rectangle((sx - 1, sy - 1, sx + shot.width, sy + shot.height), outline=BORDER)
    img.paste(shot, (sx, sy))
    caption = "captured by Ghost on its hidden desktop - never on your screen"
    d.text((sx, sy + shot.height + 12), caption, font=SANS, fill=MUTED)
    if audit is not None:
        d.text((sx, sy + shot.height + 36), f"your foreground: {audit} synthetic changes during the run",
               font=SANS, fill=GREEN if audit == 0 else AMBER)
    return img


SW = 600
S_HEAD = font(["seguisb.ttf", "DejaVuSans-Bold.ttf"], 26)
S_MONO = font(["consola.ttf", "DejaVuSansMono.ttf"], 25)
S_MONO_B = font(["consolab.ttf", "DejaVuSansMono-Bold.ttf"], 25)
S_SANS = font(["segoeui.ttf", "DejaVuSans.ttf"], 22)
S_PAD = 24


def wrap(d, text, fnt, width):
    lines, line = [], ""
    for word in text.split():
        trial = f"{line} {word}".strip()
        if line and d.textlength(trial, font=fnt) > width:
            lines.append(line)
            line = word
        else:
            line = trial
    return lines + [line]


def stacked_frame(step, index, total, shot, height, audit=None):
    img = Image.new("RGB", (SW, height), BG)
    d = ImageDraw.Draw(img)
    y = 20
    d.text((SW - S_PAD, y + 6), f"{index + 1}/{total}", font=S_SANS, fill=MUTED, anchor="ra")
    for line in wrap(d, step["label"], S_HEAD, SW - 2 * S_PAD - 50):
        d.text((S_PAD, y), line, font=S_HEAD, fill=TEXT)
        y += 34
    y += 10
    d.rectangle((S_PAD - 1, y - 1, S_PAD + shot.width, y + shot.height), outline=BORDER)
    img.paste(shot, (S_PAD, y))
    y += shot.height + 16
    d.text((S_PAD, y), step["call"]["name"], font=S_MONO_B, fill=BLUE)
    y += 34
    for key, value in response_lines(step["call"]["name"], step["response"]):
        color = GREEN if value is True or value == "hidden" else TEXT
        d.text((S_PAD, y), f"{key}: {fmt(value)}", font=S_MONO, fill=color)
        y += 32
    if audit is not None:
        d.text((S_PAD, y), f"your foreground: {audit} changes", font=S_MONO, fill=GREEN if audit == 0 else AMBER)
    return img


def stacked_height(steps, shots):
    probe = ImageDraw.Draw(Image.new("RGB", (SW, 10)))
    tallest = 0
    for step, shot in zip(steps, shots):
        head = 34 * len(wrap(probe, step["label"], S_HEAD, SW - 2 * S_PAD - 50))
        body = 34 + 32 * len(response_lines(step["call"]["name"], step["response"]))
        tallest = max(tallest, 20 + head + 10 + shot.height + 16 + body + 44)
    return tallest


def main(src, out, stacked=False, result_first=False):
    data = json.loads((Path(src) / "steps.json").read_text(encoding="utf-8"))
    steps = data["steps"]
    audit = data["stats"]["interference_audit"]["synthetic_foreground_changes"]
    frames, durations, shots = [], [], []
    for step in steps:
        shot = Image.open(Path(src) / step["png"]).convert("RGB")
        shot.thumbnail((SW - 48, 10_000) if stacked else (W - 408 - 32, H - 72 - 70))
        shots.append(shot)
    height = stacked_height(steps, shots) if stacked else None
    order = list(range(len(steps)))
    if result_first:
        order = order[-1:] + order[:-1]
    for i in order:
        step, shot = steps[i], shots[i]
        last = i == len(steps) - 1
        if stacked:
            frames.append(stacked_frame(step, i, len(steps), shot, height, audit if last else None))
        else:
            frames.append(frame(step, i, len(steps), shot, audit if last else None))
        durations.append(4500 if last else 2600)
    paletted = [f.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for f in frames]
    paletted[0].save(out, save_all=True, append_images=paletted[1:], duration=durations, loop=0, optimize=True)
    frames[order.index(len(steps) - 1)].save(Path(out).with_suffix(".png"))


if __name__ == "__main__":
    flags = {"--stacked", "--result-first"}
    args = [a for a in sys.argv[1:] if a not in flags]
    if len(args) != 2:
        sys.exit(__doc__)
    main(args[0], args[1], stacked="--stacked" in sys.argv, result_first="--result-first" in sys.argv)
