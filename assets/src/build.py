"""Logo assets with text as Inter outlines: python assets/src/build.py <Inter ttf dir> [out dir]"""

import sys
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

FONTS = Path(sys.argv[1])
OUT = Path(sys.argv[2] if len(sys.argv) > 2 else "assets")
OUT.mkdir(parents=True, exist_ok=True)

BOLD = TTFont(FONTS / "Inter-Bold.ttf")
MEDIUM = TTFont(FONTS / "Inter-Medium.ttf")

ACCENT_A, ACCENT_B, CARET = "#4F46E5", "#06B6D4", "#22D3EE"
THEMES = {
    "light": {"typed": "#0B1020", "ghost": "#A5B4C8", "tagline": "#475569"},
    "dark": {"typed": "#F8FAFC", "ghost": "#475A78", "tagline": "#94A3B8"},
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


def defs():
    return (
        '<defs><linearGradient id="g" x1="0" y1="1" x2="1" y2="0">'
        f'<stop offset="0" stop-color="{ACCENT_A}"/><stop offset="1" stop-color="{ACCENT_B}"/>'
        "</linearGradient></defs>"
    )


def icon(x=0, y=0, size=128):
    """A search field with typed text, caret and ghost completion above ranked suggestions."""
    s = size / 128
    return f"""<g transform="translate({x} {y}) scale({s})">
  <rect x="0.75" y="0.75" width="126.5" height="126.5" rx="29.5" fill="#0B1020" stroke="#26315A" stroke-width="1.5"/>
  <rect x="17" y="20" width="94" height="28" rx="14" fill="#161E3A" stroke="#2B3765" stroke-width="1.5"/>
  <rect x="29" y="30" width="27" height="8" rx="4" fill="#E2E8F0"/>
  <rect x="60" y="26.5" width="3.5" height="15" rx="1.75" fill="{CARET}"/>
  <rect x="67.5" y="30" width="30" height="8" rx="4" fill="#E2E8F0" opacity="0.3"/>
  <rect x="22" y="60" width="84" height="11" rx="5.5" fill="url(#g)"/>
  <rect x="22" y="79" width="66" height="11" rx="5.5" fill="url(#g)" opacity="0.62"/>
  <rect x="22" y="98" width="46" height="11" rx="5.5" fill="url(#g)" opacity="0.36"/>
</g>"""


def wordmark(x, baseline, size, theme):
    """`str` typed, a caret, and `ato` as the ghost completion."""
    colors = THEMES[theme]
    typed, typed_w = text_path(BOLD, "str", x, baseline, size, -0.02)
    gap = size * 0.06
    caret_x = x + typed_w + gap
    caret_w = size * 0.055
    ghost_x = caret_x + caret_w + gap
    ghost, ghost_w = text_path(BOLD, "ato", ghost_x, baseline, size, -0.02)
    cap = size * 0.78
    parts = [
        f'<path d="{typed}" fill="{colors["typed"]}"/>',
        f'<rect x="{caret_x:.2f}" y="{baseline - cap:.2f}" width="{caret_w:.2f}" height="{cap * 1.12:.2f}" '
        f'rx="{caret_w / 2:.2f}" fill="{CARET}"/>',
        f'<path d="{ghost}" fill="{colors["ghost"]}"/>',
    ]
    return "\n".join(parts), ghost_x + ghost_w - x


def svg(width, height, body, label):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}" '
        f'role="img" aria-label="{label}">\n{defs()}\n{body}\n</svg>\n'
    )


TAGLINE = "Serverless autocompletion for Rust and Python"

(OUT / "logo.svg").write_text(svg(128, 128, icon(), "strato"))

for theme in THEMES:
    mark, mark_w = wordmark(172, 98, 84, theme)
    tag, tag_w = text_path(MEDIUM, TAGLINE, 176, 142, 22)
    width = int(max(172 + mark_w, 176 + tag_w) + 24)
    body = f'{icon(16, 24)}\n{mark}\n<path d="{tag}" fill="{THEMES[theme]["tagline"]}"/>'
    (OUT / f"banner-{theme}.svg").write_text(svg(width, 176, body, f"strato: {TAGLINE.lower()}"))

# Wordmark alone, and icon plus wordmark, for places where the banner's tagline does not fit.
for theme in THEMES:
    size, pad = 96, 6
    baseline = pad + size * 0.78
    mark, mark_w = wordmark(pad, baseline, size, theme)
    height = int(baseline + size * 0.12 + pad)
    (OUT / f"wordmark-{theme}.svg").write_text(svg(int(mark_w + 2 * pad), height, mark, "strato"))
    size = 64
    baseline = 48 + size * 0.78 / 2
    mark, mark_w = wordmark(120, baseline, size, theme)
    body = f"{icon(0, 0, 96)}\n{mark}"
    (OUT / f"lockup-{theme}.svg").write_text(svg(int(120 + mark_w + 8), 96, body, "strato"))

mark, mark_w = wordmark(0, 0, 150, "dark")
tag, tag_w = text_path(MEDIUM, TAGLINE, 0, 0, 36)
kinds, kinds_w = text_path(MEDIUM, "exact · prefix · infix · abbreviation · spelling · decomposition · semantic", 0, 0, 23)
block_w = max(mark_w, tag_w, kinds_w)
left = (1280 - (240 + 64 + block_w)) / 2
social = f"""<rect width="1280" height="640" fill="#0B1020"/>
<radialGradient id="glow" cx="0.25" cy="0.35" r="0.8"><stop offset="0" stop-color="#1E1B4B"/><stop offset="1" stop-color="#0B1020"/></radialGradient>
<rect width="1280" height="640" fill="url(#glow)"/>
{icon(left, 200, 240)}
<g transform="translate({left + 304} 328)">{mark}</g>
<g transform="translate({left + 308} 390)"><path d="{tag}" fill="#94A3B8"/></g>
<g transform="translate({left + 308} 446)"><path d="{kinds}" fill="{CARET}"/></g>"""
(OUT / "social-preview.svg").write_text(svg(1280, 640, social, "strato"))
print("written to", OUT)
