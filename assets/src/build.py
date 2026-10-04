"""Brand assets with text as outlines: python assets/src/build.py <font dir> [out dir]

The font dir holds the variable fonts SpaceGrotesk.ttf and SourceSans3.ttf (SIL Open Font License)."""

import sys
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

FONTS = Path(sys.argv[1])
OUT = Path(sys.argv[2] if len(sys.argv) > 2 else "assets")
OUT.mkdir(parents=True, exist_ok=True)


def instance(name, weight):
    return instantiateVariableFont(TTFont(FONTS / name), {"wght": weight})


BOLD = instance("SpaceGrotesk.ttf", 700)
MEDIUM = instance("SourceSans3.ttf", 500)
REGULAR = instance("SourceSans3.ttf", 400)

INK, PAPER, ACCENT = "#111113", "#F6F5F1", "#F25C05"
THEMES = {
    "light": {"text": INK, "tagline": "#5B5B63"},
    "dark": {"text": PAPER, "tagline": "#A3A1A8"},
}


def text_path(font, text, x, baseline, size, tracking=0.0):
    """SVG path data for `text` and its advance width, in user units."""
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    scale = size / font["head"].unitsPerEm
    pen = SVGPathPen(glyphs, ntos=lambda v: f"{v:.1f}".rstrip("0").rstrip("."))
    cursor = x
    for ch in text:
        name = cmap[ord(ch)]
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, cursor, baseline)))
        cursor += glyphs[name].width * scale + tracking * size
    return pen.getCommands(), cursor - x - tracking * size


def wordmark(x, baseline, size, theme):
    """`complet` in the text colour, then `r` and a square full stop in the accent."""
    tracking = -0.04
    head, head_w = text_path(BOLD, "complet", x, baseline, size, tracking)
    r_x = x + head_w + tracking * size
    tail, tail_w = text_path(BOLD, "r", r_x, baseline, size)
    dot = size * 0.17
    dot_x = r_x + tail_w + size * 0.04
    parts = [
        f'<path d="{head}" fill="{THEMES[theme]["text"]}"/>',
        f'<path d="{tail}" fill="{ACCENT}"/>',
        f'<rect x="{dot_x:.2f}" y="{baseline - dot:.2f}" width="{dot:.2f}" height="{dot:.2f}" fill="{ACCENT}"/>',
    ]
    return "\n".join(parts), dot_x + dot - x


def svg(width, height, body, label):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}" '
        f'role="img" aria-label="{label}">\n{body}\n</svg>\n'
    )


TAGLINE = "Serverless autocompletion for Rust and Python"

if __name__ == "__main__":
    # The full stop alone, for favicons and the docs header.
    (OUT / "logo.svg").write_text(svg(128, 128, f'<rect x="28" y="28" width="72" height="72" fill="{ACCENT}"/>', "completr"))

    for theme in THEMES:
        mark, mark_w = wordmark(16, 92, 96, theme)
        tag, tag_w = text_path(MEDIUM, TAGLINE, 20, 140, 24)
        width = int(max(16 + mark_w, 20 + tag_w) + 16)
        body = f'{mark}\n<path d="{tag}" fill="{THEMES[theme]["tagline"]}"/>'
        (OUT / f"banner-{theme}.svg").write_text(svg(width, 164, body, f"completr: {TAGLINE.lower()}"))

        size, pad = 96, 6
        baseline = pad + size * 0.74
        mark, mark_w = wordmark(pad, baseline, size, theme)
        height = int(baseline + size * 0.24 + pad)
        (OUT / f"wordmark-{theme}.svg").write_text(svg(int(mark_w + 2 * pad), height, mark, "completr"))

    mark, mark_w = wordmark(0, 0, 160, "dark")
    tag, tag_w = text_path(MEDIUM, TAGLINE, 0, 0, 38)
    kinds, kinds_w = text_path(MEDIUM, "exact · prefix · infix · abbreviation · spelling · decomposition · semantic", 0, 0, 24)
    left = (1280 - max(mark_w, tag_w, kinds_w)) / 2
    social = f"""<rect width="1280" height="640" fill="{INK}"/>
<g transform="translate({left} 330)">{mark}</g>
<g transform="translate({left + 4} 400)"><path d="{tag}" fill="#A3A1A8"/></g>
<g transform="translate({left + 4} 456)"><path d="{kinds}" fill="{ACCENT}"/></g>"""
    (OUT / "social-preview.svg").write_text(svg(1280, 640, social, "completr"))
    # The docs site serves its own copies of the logo and banners.
    DOCS = Path("docs/assets")
    if DOCS.is_dir():
        for name in ("logo.svg", "wordmark-dark.svg", "banner-light.svg", "banner-dark.svg"):
            (DOCS / name).write_text((OUT / name).read_text())
    print("written to", OUT)
