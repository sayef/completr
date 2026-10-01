"""Animated demo from real results: python assets/src/demo_data.py && python assets/src/demo.py <Inter ttf dir>"""

import json
import sys
from pathlib import Path

from build import BOLD, CARET, MEDIUM, text_path

FONTS = Path(sys.argv[1])
REGULAR = __import__("fontTools.ttLib", fromlist=["TTFont"]).TTFont(FONTS / "Inter-Regular.ttf")
scenes = json.load(open(Path(__file__).with_name("demo.json")))
FRAME = int(__import__("os").environ.get("FRAME", "-1"))

W, H = 720, 380
SCENE, TYPE, SHOW = 3.4, 1.1, 1.25  # seconds per scene, typing time, results appear
TOTAL = SCENE * len(scenes)
KIND = {
    "exact": "#22C55E", "prefix": "#06B6D4", "abbreviation": "#A855F7",
    "infix": "#F59E0B", "fuzzy": "#F43F5E", "semantic": "#6366F1",
}


def t(sec):
    return f"{min(max(sec / TOTAL, 0), 1):.4f}"


def window(start, end, fade=0.2):
    """Opacity keyframes visible from `start` to `end` seconds of the loop."""
    if FRAME >= 0:
        return '<set attributeName="opacity" to="1"/>' if start <= FRAME * SCENE + SHOW + 0.5 < end else ""
    keys = ["0", t(start), t(start + fade), t(end - fade), t(end), "1"]
    return f'<animate attributeName="opacity" dur="{TOTAL}s" repeatCount="indefinite" keyTimes="{";".join(keys)}" values="0;0;1;1;0;0"/>'


parts = [
    f'<rect width="{W}" height="{H}" rx="18" fill="#0B1020"/>',
    '<rect x="28" y="28" width="664" height="56" rx="28" fill="#161E3A" stroke="#2B3765" stroke-width="1.5"/>',
    '<circle cx="62" cy="54" r="9" fill="none" stroke="#64748B" stroke-width="2.5"/><line x1="68.5" y1="60.5" x2="75" y2="67" stroke="#64748B" stroke-width="2.5" stroke-linecap="round"/>',
]
for n, scene in enumerate(scenes):
    start = n * SCENE
    query, width = text_path(REGULAR, scene["query"], 92, 63, 22)
    clip = f"typed{n}"
    widths = f"0;0;{width + 2:.1f};{width + 2:.1f}" if FRAME < 0 else ";".join([f"{width + 2:.1f}"] * 4)
    keys = f'0;{t(start + 0.15)};{t(start + TYPE)};1'
    parts.append(
        f'<clipPath id="{clip}"><rect x="92" y="30" height="52" width="0">'
        f'<animate attributeName="width" dur="{TOTAL}s" repeatCount="indefinite" keyTimes="{keys}" values="{widths}"/>'
        f"</rect></clipPath>"
    )
    rows = []
    for i, (title, kind, _) in enumerate(scene["hits"]):
        y = 112 + i * 52
        label, lw = text_path(MEDIUM, title, 52, y + 30, 19)
        badge, bw = text_path(MEDIUM, kind, 0, 0, 13)
        bx = 668 - bw - 24
        rows.append(
            f'<rect x="36" y="{y}" width="648" height="44" rx="10" fill="{"#161E3A" if i == 0 else "none"}"/>'
            f'<path d="{label}" fill="#E2E8F0"/>'
            f'<rect x="{bx:.1f}" y="{y + 11}" width="{bw + 24:.1f}" height="22" rx="11" fill="{KIND[kind]}" fill-opacity="0.18" stroke="{KIND[kind]}" stroke-opacity="0.6"/>'
            f'<g transform="translate({bx + 12:.1f} {y + 26.5})"><path d="{badge}" fill="{KIND[kind]}"/></g>'
        )
    caption, cw = text_path(MEDIUM, scene["caption"], 0, 0, 14)
    parts.append(
        f'<g opacity="0">{window(start, start + SCENE)}'
        f'<g clip-path="url(#{clip})"><path d="{query}" fill="#F8FAFC"/></g>'
        f'<rect y="42" width="2.5" height="26" rx="1.25" fill="{CARET}">'
        f'<animate attributeName="x" dur="{TOTAL}s" repeatCount="indefinite" keyTimes="{keys}" values="93;93;{95 + width:.1f};{95 + width:.1f}"/>'
        f'<animate attributeName="opacity" dur="0.9s" repeatCount="indefinite" values="1;1;0;0" keyTimes="0;0.5;0.55;1"/></rect>'
        f'<g transform="translate({W - 44 - cw:.1f} 340)"><path d="{caption}" fill="#64748B"/></g>'
        f'<g opacity="0">{window(start + SHOW, start + SCENE, 0.15)}{"".join(rows)}</g>'
        "</g>"
    )
dots = "".join(
    f'<circle cx="{44 + i * 16}" cy="336" r="4" fill="#334155"/>'
    f'<circle cx="{44 + i * 16}" cy="336" r="4" fill="{CARET}" opacity="0">{window(i * SCENE, (i + 1) * SCENE, 0.1)}</circle>'
    for i in range(len(scenes))
)
svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" '
    f'aria-label="completr completing queries: prefix, abbreviation, spelling correction, word decomposition and infix">'
    + "".join(parts) + dots + "</svg>\n"
)
if FRAME >= 0:
    import re
    svg = svg.replace('opacity="0"><set attributeName="opacity" to="1"/>', 'opacity="1">')
    svg = re.sub(r'<rect x="92" y="30" height="52" width="0"><animate attributeName="width"[^>]*values="([0-9.]+);[^"]*"/>',
                 r'<rect x="92" y="30" height="52" width="\1">', svg)
    svg = re.sub(r'<rect y="42"([^>]*)><animate attributeName="x"[^>]*values="[0-9.]+;[0-9.]+;([0-9.]+);[^"]*"/>',
                 r'<rect y="42" x="\2"\1>', svg)
Path("assets/demo.svg" if FRAME < 0 else f"assets/frame{FRAME}.svg").write_text(svg)
if FRAME < 0 and Path("docs/assets").is_dir():
    Path("docs/assets/demo.svg").write_text(svg)
print(len(svg) // 1024, "KB")
