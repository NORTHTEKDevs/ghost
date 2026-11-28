"""Draws the README banner and the measured-claims card in the demo GIF's
palette (GitHub dark, the green that means "verified"), with one type pairing
of their own: Bahnschrift Bold for the name and the numbers, Segoe UI Light
for the words. Both are drawn at 1200px and shown at 520px, the demo's width,
so their text stays readable when GitHub shrinks them to a 400px phone column.

    python scripts/readme-art.py        # writes assets/banner.png, assets/proof.png

The numbers on the proof card come from docs/background-mode.md and
docs/benchmarks.md; change them there first.
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
BG, BORDER = "#0d1117", "#30363d"
TEXT, MUTED, GREEN = "#e6edf3", "#8b949e", "#3fb950"
W, PAD = 1200, 72


def first_font(names, size):
    for name in names:
        for base in ("C:/Windows/Fonts/", "/usr/share/fonts/truetype/dejavu/", ""):
            try:
                return ImageFont.truetype(base + name, size)
            except OSError:
                pass
    raise SystemExit(f"none of {names} found")


def display(size):
    f = first_font(["bahnschrift.ttf", "DejaVuSansCondensed-Bold.ttf"], size)
    try:
        f.set_variation_by_name("Bold")
    except (OSError, ValueError, AttributeError):
        pass  # not a variable font: already bold
    return f


def light(size):
    return first_font(["segoeuil.ttf", "DejaVuSans-ExtraLight.ttf", "DejaVuSans.ttf"], size)


def fit(draw, text, maker, size, width):
    """The largest font from `maker` at or below `size` that fits `width`."""
    while size > 10:
        f = maker(size)
        if draw.textlength(text, font=f) <= width:
            return f
        size -= 2
    return maker(size)


def banner():
    H = 560
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    icon = Image.open(ROOT / "assets/icon.png").convert("RGBA").resize((136, 136), Image.LANCZOS)
    im.paste(icon, (PAD - 12, PAD - 4), icon)
    d.text((PAD + 150, PAD + 64), "Ghost", font=display(150), fill=TEXT, anchor="lm")
    lines = ["AI agents use the apps on your computer,", "in the background."]
    f = fit(d, lines[0], light, 62, W - 2 * PAD)
    y = 268
    for line in lines:
        d.text((PAD, y), line, font=f, fill=TEXT)
        y += 76
    d.text((PAD, y + 8), "Every action verified.", font=display(66), fill=GREEN)
    im.save(ROOT / "assets/banner.png", optimize=True)


def proof():
    rows = [
        ("2,949 of ~2,950", ["keystrokes typed during six agent runs", "stayed in the person's window"]),
        ("0", ["focus losses in a 160-action soak,", "and 0 unverified results"]),
        ("14 of 14", ["benchmark tasks passed, scored by", "re-observing the app"]),
    ]
    NUM, LINE, GAP = 118, 64, 56
    probe = ImageDraw.Draw(Image.new("RGB", (1, 1)))
    words = min(fit(probe, l, light, 54, W - 2 * PAD).size for _, ls in rows for l in ls)
    lf = light(words)
    row_h = NUM + 20 + LINE * 2
    H = PAD * 2 + row_h * len(rows) + GAP * (len(rows) - 1)
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    big = display(NUM)
    top = PAD
    for i, (num, lines) in enumerate(rows):
        if i:
            d.line((PAD, top - GAP // 2, W - PAD, top - GAP // 2), fill=BORDER, width=2)
        d.text((PAD, top), num, font=big, fill=GREEN, anchor="la")
        y = top + NUM + 20
        for line in lines:
            d.text((PAD, y), line, font=lf, fill=TEXT, anchor="la")
            y += LINE
        top += row_h + GAP
    im.save(ROOT / "assets/proof.png", optimize=True)


if __name__ == "__main__":
    banner()
    proof()
    print("wrote assets/banner.png, assets/proof.png")
