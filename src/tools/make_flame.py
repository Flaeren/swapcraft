"""Renders the voidflare flame mark (variant C, "Torcia") from its SVG paths.

    python tools/make_flame.py

Writes hub/assets/flame_{dark,light}.rgba (raw RGBA, for the hub) and prints the base64 PNGs that go
into word-to-pdf/theme.py and dwg-to-pdf/theme.py (tkinter has no anti-aliasing of its own).
The same paths are in tools/flame-mockups.html (symbol "fc").
"""
import base64
import io
import re
from pathlib import Path

from PIL import Image, ImageDraw

OUTER = ("M50 98C31 98 20 85 22 69C24 55 34 47 38 35C40 27 38 16 43 4C49 15 58 22 61 34C62 40 61 45 64 49"
         "C68 43 68 37 66 30C78 43 82 58 80 71C78 87 67 98 50 98Z")
CORE = "M50 92C40 92 33 85 34 76C35 68 41 63 44 54C46 59 49 61 51 59C50 51 54 45 58 39C57 51 66 57 66 72C66 85 60 92 50 92Z"
CORE_SCALE, CORE_DX, CORE_DY = 0.85, 7.5, 13.8   # the core is the flame's own shape, shrunk into its body
LOOKS = {"DARK": ("#6366f1", "#a5b4fc"), "LIGHT": ("#4f46e5", "#c7d2fe")}
TK_H = 30          # height in px of the tkinter PNG
HUB_H = 84         # height in px of the hub texture (drawn at 28 logical px, so 3x for sharp HiDPI)
SS = 8             # supersampling


def flatten(d: str, scale: float = 1.0, dx: float = 0.0, dy: float = 0.0, steps: int = 24):
    """Points of an absolute M/C/Z path, cubic curves sampled."""
    nums = [float(v) for v in re.findall(r"-?\d+\.?\d*", d)]
    pts, i, cur = [], 0, None
    for cmd in re.findall(r"[MCZ]", d):
        if cmd == "M":
            cur = (nums[i] * scale + dx, nums[i + 1] * scale + dy); i += 2; pts.append(cur)
        elif cmd == "C":
            p1, p2, p3 = [(nums[i + k] * scale + dx, nums[i + k + 1] * scale + dy) for k in (0, 2, 4)]; i += 6
            for s in range(1, steps + 1):
                t = s / steps; u = 1 - t
                pts.append(tuple(u ** 3 * a + 3 * u * u * t * b + 3 * u * t * t * c + t ** 3 * e
                                 for a, b, c, e in zip(cur, p1, p2, p3)))
            cur = p3
    return pts


def render(outer: str, core: str, height: int) -> Image.Image:
    o, c = flatten(OUTER), flatten(CORE, CORE_SCALE, CORE_DX, CORE_DY)
    x0, x1 = min(p[0] for p in o), max(p[0] for p in o)
    y0, y1 = min(p[1] for p in o), max(p[1] for p in o)
    scale = height * SS / (y1 - y0)
    w = round((x1 - x0) * scale / SS)
    big = Image.new("RGBA", (w * SS, height * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    for pts, colour in ((o, outer), (c, core)):
        d.polygon([((x - x0) * scale, (y - y0) * scale) for x, y in pts], fill=colour)
    return big.resize((w, height), Image.LANCZOS)


if __name__ == "__main__":
    assets = Path(__file__).resolve().parent.parent / "hub" / "assets"
    assets.mkdir(exist_ok=True)
    for name, (outer, core) in LOOKS.items():
        hub = render(outer, core, HUB_H)
        (assets / f"flame_{name.lower()}.rgba").write_bytes(hub.tobytes())
        buf = io.BytesIO()
        render(outer, core, TK_H).save(buf, "PNG", optimize=True)
        print(f'FLAME_{name} = "{base64.b64encode(buf.getvalue()).decode()}"')
    print(f"hub texture size: {hub.size[0]}x{hub.size[1]}")
