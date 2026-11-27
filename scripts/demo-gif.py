"""Builds assets/demo.gif from the output of scripts/demo-capture.mjs.

    python scripts/demo-gif.py <capture-dir> assets/demo.gif

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


def main(src, out):
    data = json.loads((Path(src) / "steps.json").read_text(encoding="utf-8"))
    steps = data["steps"]
    audit = data["stats"]["interference_audit"]["synthetic_foreground_changes"]
    frames, durations = [], []
    for i, step in enumerate(steps):
        shot = Image.open(Path(src) / step["png"]).convert("RGB")
        shot.thumbnail((W - 408 - 32, H - 72 - 70))
        last = i == len(steps) - 1
        frames.append(frame(step, i, len(steps), shot, audit if last else None))
        durations.append(4500 if last else 2600)
    paletted = [f.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for f in frames]
    paletted[0].save(out, save_all=True, append_images=paletted[1:], duration=durations, loop=0, optimize=True)
    frames[-1].save(Path(out).with_suffix(".png"))


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
