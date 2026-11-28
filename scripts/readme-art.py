"""Draws the README banner and the measured-claims card in the demo GIF's
palette (GitHub dark, Segoe UI Semibold + Consolas), sized so their text stays
readable when GitHub shrinks them to a 400px phone column.

    python scripts/readme-art.py        # writes assets/banner.png, assets/proof.png

The numbers on the proof card come from docs/background-mode.md and
docs/benchmarks.md; change them there first.
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
BG, PANEL, BORDER = "#0d1117", "#161b22", "#30363d"
TEXT, MUTED, BLUE, GREEN = "#e6edf3", "#8b949e", "#58a6ff", "#3fb950"


def font(names, size):
    for name in names:
        for base in ("C:/Windows/Fonts/", "/usr/share/fonts/truetype/dejavu/", ""):
            try:
                return ImageFont.truetype(base + name, size)
            except OSError:
                pass
    raise SystemExit(f"none of {names} found")


def sans_b(size):
    return font(["seguisb.ttf", "DejaVuSans-Bold.ttf"], size)


def sans(size):
    return font(["segoeui.ttf", "DejaVuSans.ttf"], size)


def mono(size):
    return font(["consola.ttf", "DejaVuSansMono.ttf"], size)


def banner():
    W, H, PAD = 1200, 500, 64
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    icon = Image.open(ROOT / "assets/icon.png").convert("RGBA").resize((112, 112), Image.LANCZOS)
    im.paste(icon, (PAD, PAD), icon)
    d.text((PAD + 140, PAD + 56), "Ghost", font=sans_b(96), fill=TEXT, anchor="lm")
    tag = sans(46)
    d.text((PAD, 228), "AI agents use the apps on your computer,", font=tag, fill=TEXT)
    d.text((PAD, 284), "in the background. Every action verified.", font=tag, fill=TEXT)
    # The receipt every ghost_act call returns, as in the demo.
    y, box_h = 372, 76
    d.rounded_rectangle((PAD, y, W - PAD, y + box_h), radius=14, fill=PANEL, outline=BORDER, width=2)
    m = mono(40)
    x = PAD + 28
    for key, value in (("verified", "true"), ("your foreground", "0 changes")):
        d.text((x, y + box_h // 2), f"{key}: ", font=m, fill=TEXT, anchor="lm")
        x += d.textlength(f"{key}: ", font=m)
        d.text((x, y + box_h // 2), value, font=m, fill=GREEN, anchor="lm")
        x += d.textlength(value, font=m) + 44
    im.save(ROOT / "assets/banner.png", optimize=True)


def proof():
    rows = [
        ("2,949", "of about 2,950 keystrokes a person typed during six agent runs stayed in their window"),
        ("0", "focus losses in a 160-action soak, with 0 unverified results"),
        ("14/14", "benchmark tasks passed when scored by re-observing the app"),
    ]
    W, PAD, NUM_W = 1200, 56, 280
    big, label = sans_b(84), sans(40)
    text_x = PAD + NUM_W
    text_w = W - PAD - text_x
    probe = ImageDraw.Draw(Image.new("RGB", (1, 1)))

    def wrap(text):
        lines, line = [], ""
        for word in text.split():
            trial = f"{line} {word}".strip()
            if probe.textlength(trial, font=label) <= text_w:
                line = trial
            else:
                lines.append(line)
                line = word
        return lines + [line]

    wrapped = [wrap(t) for _, t in rows]
    LINE, GAP = 52, 44
    heights = [max(len(w) * LINE, 100) for w in wrapped]
    H = PAD * 2 + sum(heights) + GAP * (len(rows) - 1)
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    top = PAD
    for i, ((num, _), lines, h) in enumerate(zip(rows, wrapped, heights)):
        if i:
            d.line((PAD, top - GAP // 2, W - PAD, top - GAP // 2), fill=BORDER, width=2)
        mid = top + h // 2
        d.text((PAD, mid), num, font=big, fill=GREEN, anchor="lm")
        y = mid - (len(lines) * LINE) // 2 + LINE // 2
        for line in lines:
            d.text((text_x, y), line, font=label, fill=TEXT, anchor="lm")
            y += LINE
        top += h + GAP
    im.save(ROOT / "assets/proof.png", optimize=True)


if __name__ == "__main__":
    banner()
    proof()
    print("wrote assets/banner.png, assets/proof.png")
