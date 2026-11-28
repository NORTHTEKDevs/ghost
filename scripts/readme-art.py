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
from PIL import Image, ImageDraw, ImageFont, ImageOps

ROOT = Path(__file__).resolve().parent.parent
BG, BORDER = "#0d1117", "#30363d"
TEXT, MUTED = "#e6edf3", "#8b949e"
GREEN = "#5ce6c0"  # Ghost mint: "verified" in every panel
PRIMER_GREEN = (0x3f, 0xb9, 0x50)  # what demo-gif.py drew the verified lines in
W, PAD = 1200, 48  # 48 of 1200 is ~21px at 520, the demo frame's inset


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
    return first_font(["segoeuisl.ttf", "segoeuil.ttf", "DejaVuSans.ttf"], size)


def mono(size):
    return first_font(["consola.ttf", "DejaVuSansMono.ttf"], size)


GRID, DOT = 24, "#222a35"


def textured(w, h):
    """The panel ground: GitHub dark with a faint dot grid, the texture every
    piece of README art shares."""
    im = Image.new("RGB", (w, h), BG)
    d = ImageDraw.Draw(im)
    for y in range(GRID // 2, h, GRID):
        for x in range(GRID // 2, w, GRID):
            d.rectangle((x, y, x + 1, y + 1), fill=DOT)
    return im


def window_mark(d, x0, y0, w, h, col="#2a3340", width=3):
    """A window outline with three title-bar dots: the hidden desktop, Ghost's
    signature mark on every panel."""
    d.rounded_rectangle((x0, y0, x0 + w, y0 + h), radius=max(6, w // 30), outline=col, width=width)
    bar = max(10, h // 7)
    d.line((x0, y0 + bar, x0 + w, y0 + bar), fill=col, width=width)
    r = max(3, bar // 5)
    for k in range(3):
        cx = x0 + w - bar * (k + 1) * 3 // 4 - r
        d.ellipse((cx - r, y0 + bar // 2 - r, cx + r, y0 + bar // 2 + r), outline=col, width=width)


def corner_mark(d, w):
    """The window mark, small, in a panel's top-right corner."""
    window_mark(d, w - PAD - 96, 22, 96, 64, width=2)


OFFSET = 28  # how far the window behind each panel shows, at 1200px


def stacked(panel):
    """The panel as a window with a second window behind it, offset down and
    right: the hidden desktop under the one you see. Transparent around the
    two windows, so the page shows through."""
    w, h = panel.size
    out = Image.new("RGBA", (w + OFFSET, h + OFFSET), (0, 0, 0, 0))
    d = ImageDraw.Draw(out)
    d.rectangle((OFFSET, OFFSET, w + OFFSET - 1, h + OFFSET - 1), fill="#161b22", outline=BORDER, width=2)
    out.paste(panel.convert("RGBA"), (0, 0))
    d.rectangle((0, 0, w - 1, h - 1), outline=BORDER, width=2)
    return out


def glyph(size):
    """The ghost from assets/icon.png, in white, without its square tile."""
    icon = Image.open(ROOT / "assets/icon.png").convert("RGBA").resize((size, size), Image.LANCZOS)
    mask = Image.new("L", icon.size, 0)
    px, mp = icon.load(), mask.load()
    for y in range(size):
        for x in range(size):
            r, g, b, a = px[x, y]
            # The glyph is the blue part; the tile is near-black.
            mp[x, y] = a if b > 120 and b > r + 40 else 0
    white = Image.new("RGBA", icon.size, TEXT)
    white.putalpha(mask)
    return white.crop(mask.getbbox())


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
    im = textured(W, H)
    d = ImageDraw.Draw(im)
    # The hidden desktop: two offset window outlines in the gutter beside the
    # wordmark, clear of the tagline, where Ghost runs the apps it starts.
    window_mark(d, 820, 34, 300, 168, col="#1d242e")
    window_mark(d, 790, 58, 300, 168, col="#2a3340")
    mark = glyph(190)
    im.paste(mark, (PAD, PAD + 70 - mark.height // 2), mark)
    d.text((PAD + mark.width + 26, PAD + 64), "Ghost", font=display(150), fill=TEXT, anchor="lm")
    lines = ["AI agents use the apps on your computer,", "in the background."]
    f = fit(d, lines[0], light, 62, W - 2 * PAD)
    y = 268
    for line in lines:
        d.text((PAD, y), line, font=f, fill=TEXT)
        y += 76
    d.text((PAD, y + 8), "Every action verified.", font=display(66), fill=GREEN)
    stacked(im).save(ROOT / "assets/banner.png", optimize=True)


def proof():
    rows = [
        ("0", ["focus losses in a 160-action soak,", "and 0 unverified results"]),
        ("14 of 14", ["benchmark tasks passed, scored by", "re-observing the app"]),
        ("2,949 of ~2,950", ["keystrokes a person typed during six", "agent runs stayed in their window"]),
    ]
    NUM, LINE, GAP = 118, 64, 56
    probe = ImageDraw.Draw(Image.new("RGB", (1, 1)))
    words = min(fit(probe, l, light, 54, W - 2 * PAD).size for _, ls in rows for l in ls)
    lf = light(words)
    row_h = NUM + 20 + LINE * 2
    H = PAD * 2 + row_h * len(rows) + GAP * (len(rows) - 1)
    im = textured(W, H)
    d = ImageDraw.Draw(im)
    corner_mark(d, W)
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
    stacked(im).save(ROOT / "assets/proof.png", optimize=True)


def title_inset():
    """assets/demo-stacked.gif with the captured window's title bar repeated
    under the window at twice its size (nearest-neighbour, so it is the same
    pixels, only larger). The title is what proves the click in the last
    step, and at phone width the original is about 7px tall."""
    src = Image.open(ROOT / "assets/demo-stacked.gif")
    frames, durations = [], []
    # The step captions demo-gif.py drew into each frame, in frame order
    # (result first). Re-set here in the art's display face.
    captions = ["Click again. The window title proves it.",
                "Launch an app. Ghost starts it on a hidden desktop.",
                "Type into a field, in the background.",
                "Click a button. Ghost checks it took effect."]
    BAND, SCALE, CROP_W, CROP_H = 84, 2, 262, 26
    for i in range(src.n_frames):
        src.seek(i)
        durations.append(src.info.get("duration", 2600))
        f = src.convert("RGB").copy()
        w, h = f.size
        # The window is the only thing light across the whole middle of the
        # frame (the headline is light text on dark, with gaps).
        def window_row(y):
            return all(sum(f.getpixel((x, y))) > 500 for x in range(w * 3 // 10, w * 7 // 10, 8))
        rows = [y for y in range(h) if window_row(y)]
        top = rows[0]
        left = next(x for x in range(w) if sum(f.getpixel((x, top + 4))) > 600)
        # Walk down the window's light left margin to the end of its client
        # area, then through the dark frame under it.
        bottom = top
        while bottom + 1 < h and sum(f.getpixel((left + 3, bottom + 1))) > 500:
            bottom += 1
        while bottom + 1 < h and sum(f.getpixel((left + 3, bottom + 1))) < 60 and \
                sum(f.getpixel((left + 3, bottom + 1))) != sum(f.getpixel((2, bottom + 1))):
            bottom += 1
        head = Image.new("RGB", (w, top - 4), BG)
        hd = ImageDraw.Draw(head)
        words, lines, line = captions[i].split(), [], ""
        hf = display(27)
        for word in words:
            trial = f"{line} {word}".strip()
            if hd.textlength(trial, font=hf) <= w - 2 * left:
                line = trial
            else:
                lines.append(line)
                line = word
        lines.append(line)
        ly = (top - 4 - 34 * len(lines)) // 2 + 4
        for text in lines:
            hd.text((left, ly), text, font=hf, fill=TEXT)
            ly += 34
        f.paste(head, (0, 0))
        # Contrast-stretched so the proof reads; the pixels are the capture's.
        crop = ImageOps.autocontrast(f.crop((left + 2, top, left + 2 + CROP_W, top + CROP_H)), cutoff=1)
        zoom = crop.resize((CROP_W * SCALE, CROP_H * SCALE), Image.NEAREST)
        out = Image.new("RGB", (w, h + BAND), BG)
        grid = textured(w, h + BAND)
        out.paste(f.crop((0, 0, w, bottom + 1)), (0, 0))
        y = bottom + 1 + (BAND - CROP_H * SCALE) // 2
        out.paste(zoom, (left, y))
        d = ImageDraw.Draw(out)
        d.rectangle((left - 1, y - 1, left + CROP_W * SCALE, y + CROP_H * SCALE), outline=BORDER)
        out.paste(f.crop((0, bottom + 1, w, h)), (0, bottom + 1 + BAND))
        bgc = tuple(int(BG[i:i + 2], 16) for i in (1, 3, 5))
        mint = tuple(int(GREEN[i:i + 2], 16) for i in (1, 3, 5))
        px = out.load()
        for yy in range(out.height):
            for xx in range(w):
                r, g, b = px[xx, yy]
                t = (g - bgc[1]) / (PRIMER_GREEN[1] - bgc[1])
                if t <= 0.05 or t > 1.05:
                    continue
                expect = [bgc[k] + t * (PRIMER_GREEN[k] - bgc[k]) for k in range(3)]
                if all(abs(c - e) <= 10 for c, e in zip((r, g, b), expect)):
                    px[xx, yy] = tuple(round(bgc[k] + t * (mint[k] - bgc[k])) for k in range(3))
        # The same dot grid wherever the frame shows bare panel ground.
        bg = tuple(int(BG[i:i + 2], 16) for i in (1, 3, 5))
        o, g = out.load(), grid.load()
        for yy in range(out.height):
            for xx in range(w):
                if o[xx, yy] == bg:
                    o[xx, yy] = g[xx, yy]
        big = out.resize((out.width * 2, out.height * 2), Image.NEAREST)
        frames.append(stacked(big))
    # GIF has one transparent palette entry: mark the see-through corners with
    # a colour no frame uses, then make that entry transparent.
    # One palette for every frame, so that entry has the same index in each.
    KEY = (255, 0, 255)
    flats = []
    for fr in frames:
        flat = Image.new("RGB", fr.size, KEY)
        flat.paste(fr, (0, 0), fr)
        flats.append(flat)
    sheet = Image.new("RGB", (flats[0].width, sum(f.height for f in flats)))
    y = 0
    for flat in flats:
        sheet.paste(flat, (0, y))
        y += flat.height
    palette = sheet.quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    pal = palette.getpalette()[:768]
    key_index = min(range(len(pal) // 3), key=lambda i: sum(abs(pal[i * 3 + k] - KEY[k]) for k in range(3)))
    assert tuple(pal[key_index * 3:key_index * 3 + 3]) == KEY, "transparency key lost in the palette"
    paletted = [f.quantize(palette=palette, dither=Image.Dither.NONE) for f in flats]
    paletted[0].save(ROOT / "assets/demo-stacked-title.gif", save_all=True, append_images=paletted[1:],
                     duration=durations, loop=0, disposal=2, transparency=key_index, optimize=False)


def cta():
    """The one call to action: a Quick start button in the art palette."""
    w, h = 360, 96
    im = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle((0, 0, w - 1, h - 1), radius=18, fill=GREEN)
    d.text((w // 2, h // 2 + 2), "Quick start  >", font=display(44), fill=BG, anchor="mm")
    im.save(ROOT / "assets/quick-start.png", optimize=True)


def loop():
    """The look, act, confirm loop from "How an agent uses it": three
    stations on one line, in the same panel language."""
    steps = [("look", "ghost_see"), ("act", "ghost_act"), ("confirm", "verified: true")]
    H = 340
    im = textured(W, H)
    d = ImageDraw.Draw(im)
    corner_mark(d, W)
    word, code = display(58), mono(38)
    line_y = 200
    xs = [PAD + 150 + i * (W - 2 * PAD - 300) // 2 for i in range(3)]
    d.line((xs[0], line_y, xs[2], line_y), fill=BORDER, width=4)
    for i, (label, call) in enumerate(steps):
        x = xs[i]
        if i < 2:
            mid = (xs[i] + xs[i + 1]) // 2
            d.polygon([(mid - 14, line_y - 14), (mid + 12, line_y), (mid - 14, line_y + 14)], fill=GREEN)
        d.ellipse((x - 14, line_y - 14, x + 14, line_y + 14), fill=GREEN if i == 2 else TEXT)
        d.text((x, line_y - 56), label, font=word, fill=TEXT, anchor="mm")
        d.text((x, line_y + 62), call, font=code, fill=GREEN if i == 2 else "#b1bac4", anchor="mm")
    stacked(im).save(ROOT / "assets/loop.png", optimize=True)


if __name__ == "__main__":
    banner()
    proof()
    title_inset()
    loop()
    cta()
    print("wrote assets/banner.png, assets/proof.png, assets/demo-stacked-title.gif, assets/loop.png")
