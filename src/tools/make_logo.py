"""Writes the SwapCraft logo for the README (docs/img/logo.svg): the flame, "Swap" in white, "Craft" in a gradient
of the voidflare indigos, and "by voidflare" underneath.

    python tools/make_logo.py

The mark is the flame of tools/make_flame.py (same two paths). The wordmark is set in Michroma (SIL Open
Font License 1.1, (c) 2011 The Michroma Project Authors) and turned into outlines, because an SVG shown by
GitHub cannot load web fonts. The font file is downloaded on first use and is not kept in the repository.
Needs: pip install fonttools
"""
import urllib.request
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent.parent
FONT = Path(__file__).resolve().parent / "Michroma-Regular.ttf"
FONT_URL = "https://github.com/google/fonts/raw/main/ofl/michroma/Michroma-Regular.ttf"

# From make_flame.py: the body and the core (the core is the same shape, shrunk into the body).
OUTER = ("M50 98C31 98 20 85 22 69C24 55 34 47 38 35C40 27 38 16 43 4C49 15 58 22 61 34C62 40 61 45 64 49"
         "C68 43 68 37 66 30C78 43 82 58 80 71C78 87 67 98 50 98Z")
CORE = "M50 92C40 92 33 85 34 76C35 68 41 63 44 54C46 59 49 61 51 59C50 51 54 45 58 39C57 51 66 57 66 72C66 85 60 92 50 92Z"
CORE_TRANSFORM = "translate(7.5 13.8) scale(0.85)"
FLAME_BOX = "19 3 64 96"          # x y w h around the flame
BODY, GLOW = "#6366f1", "#a5b4fc"


def wordmark(text: str, size: float, tracking: float) -> tuple[str, float]:
    """The text as one SVG path (baseline at y=0, starting at x=0) and its width."""
    font = TTFont(FONT)
    glyphs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font["hmtx"]
    scale = size / font["head"].unitsPerEm
    x, parts = 0.0, []
    for ch in text:
        name = cmap[ord(ch)]
        pen = SVGPathPen(glyphs, ntos=lambda v: f"{v:.2f}".rstrip("0").rstrip("."))
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, 0)))
        parts.append(pen.getCommands())
        x += hmtx[name][0] * scale + tracking
    return "".join(parts), x - tracking


def flame(x: float, y: float, height: float, tag: str = "") -> str:
    s = height / 96
    return (f'<g transform="translate({x} {y}) scale({s}) translate(-19 -3)"{tag}>'
            f'<path d="{OUTER}" fill="{BODY}"/>'
            f'<path d="{CORE}" fill="{GLOW}" transform="{CORE_TRANSFORM}"/></g>')


def main() -> None:
    if not FONT.exists():
        urllib.request.urlretrieve(FONT_URL, FONT)
    swap, swap_w = wordmark("Swap", size=30, tracking=1.5)
    craft, craft_w = wordmark("Craft", size=30, tracking=1.5)
    by, by_w = wordmark("by voidflare", size=9.5, tracking=1.2)
    h, pad, mark_h = 96, 28, 60
    mark_w = mark_h * 64 / 96
    text_x = pad + mark_w + 22
    craft_x = text_x + swap_w + 1.5
    width = max(craft_x + craft_w, text_x + by_w) - text_x
    total = text_x + width + pad
    base = 50.0  # baseline of SwapCraft; the byline sits under it
    logo = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{total:.0f}" height="{h}" viewBox="0 0 {total:.0f} {h}" '
            f'role="img" aria-label="SwapCraft by voidflare">\n'
            f'  <defs><linearGradient id="craft" gradientUnits="userSpaceOnUse" x1="{craft_x:.2f}" y1="0" '
            f'x2="{craft_x + craft_w:.2f}" y2="0"><stop offset="0" stop-color="#818cf8"/>'
            f'<stop offset="1" stop-color="#c7d2fe"/></linearGradient></defs>\n'
            f'  <rect width="{total:.0f}" height="{h}" fill="#121316"/>\n'
            f'  {flame(pad, (h - mark_h) / 2, mark_h)}\n'
            f'  <path transform="translate({text_x:.2f} {base})" fill="#ffffff" d="{swap}"/>\n'
            f'  <path transform="translate({craft_x:.2f} {base})" fill="url(#craft)" d="{craft}"/>\n'
            f'  <path transform="translate({text_x + 1:.2f} {base + 20})" fill="#8b8f98" d="{by}"/>\n'
            f'</svg>\n')
    (ROOT / "docs" / "img" / "logo.svg").write_text(logo, encoding="utf-8")
    print(f"logo {total:.0f}x{h}, wordmark {width:.0f} wide")


if __name__ == "__main__":
    main()
