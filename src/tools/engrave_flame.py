"""The voidflare flame (variant C silhouette) in the engraved style of engrave_icons.py.

    python tools/engrave_flame.py [OUT_DIR]

Two ways of engraving it, on the same dark tile as the app icons:
  contour  nested outlines, from the rim to the core, thicker towards the hot centre
  flow     hairlines running up the flame and converging on its tips
"""
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

import make_flame  # the silhouette paths

S = 1024
LAV = np.array([165, 180, 252], float)      # rim colour (indigo-300)
CREAM = np.array([243, 236, 255], float)    # hot core
GLOW = (99, 102, 241)                       # indigo-500


def silhouette():
    pts = make_flame.flatten(make_flame.OUTER)
    x0, x1 = min(p[0] for p in pts), max(p[0] for p in pts)
    y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
    scale = 640 / (y1 - y0)
    ox, oy = S / 2 - (x0 + x1) / 2 * scale, 505 - (y0 + y1) / 2 * scale
    poly = [(x * scale + ox, y * scale + oy) for x, y in pts]
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).polygon(poly, fill=255)
    return m, (x0 * scale + ox, x1 * scale + ox, y0 * scale + oy, y1 * scale + oy)


def tile():
    yy, xx = np.mgrid[0:S, 0:S]
    t = np.clip(np.hypot(xx - S * .5, yy - S * .45) / (S * .75), 0, 1)[..., None]
    rgb = (np.array((30, 28, 62)) * (1 - t) + np.array((10, 10, 22)) * t).astype(np.uint8)
    base = Image.fromarray(np.dstack([rgb, np.full((S, S), 255, np.uint8)]), "RGBA")
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([S * .04, S * .04, S * .96, S * .96], radius=S * .22, fill=255)
    return base, mask


def lines_alpha(phase, width, feather=1.0):
    """Anti-aliased lines at integer values of `phase`; `width` is the line width as a fraction of the spacing."""
    gy, gx = np.gradient(phase)
    grad = np.maximum(np.hypot(gx, gy), 0.004)           # phase units per pixel
    dist_px = np.abs(phase - np.round(phase)) / grad       # distance to the nearest line centre, in pixels
    half_px = width / grad / 2
    return np.clip((half_px - dist_px) / feather + 0.5, 0, 1)


def contour(mask, box):
    m = np.asarray(mask, float) / 255
    img = mask
    field = np.zeros((S, S))
    for r in (7, 18, 40):
        field += np.asarray(img.filter(ImageFilter.GaussianBlur(r)), float) / 255
    field /= 3                                             # 0 at the rim, towards 1 in the core
    yn = np.clip((np.mgrid[0:S, 0:S][0] - box[2]) / (box[3] - box[2]), 0, 1)
    heat = np.clip(field, 0, 1) ** 1.2 * (0.6 + 0.4 * yn)
    width = 0.12 + 0.70 * heat
    alpha = lines_alpha(field * 15.0, width) * m
    colour = LAV[None, None, :] * (1 - heat[..., None]) + CREAM[None, None, :] * heat[..., None]
    return alpha, colour


def core_mask():
    """The lighter inner flame of the flat mark, placed like the silhouette."""
    o = make_flame.flatten(make_flame.OUTER)
    c = make_flame.flatten(make_flame.CORE, make_flame.CORE_SCALE, make_flame.CORE_DX, make_flame.CORE_DY)
    x0, x1 = min(p[0] for p in o), max(p[0] for p in o)
    y0, y1 = min(p[1] for p in o), max(p[1] for p in o)
    scale = 640 / (y1 - y0)
    ox, oy = S / 2 - (x0 + x1) / 2 * scale, 505 - (y0 + y1) / 2 * scale
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).polygon([(x * scale + ox, y * scale + oy) for x, y in c], fill=255)
    return m


def flow(mask, box):
    m = np.asarray(mask, float) / 255
    yy, xx = np.mgrid[0:S, 0:S].astype(float)
    # row extents of the silhouette: lines follow it, so they converge on the tips and the base
    cols = m > 0.5
    left = np.where(cols.any(1), cols.argmax(1), 0).astype(float)
    right = np.where(cols.any(1), S - 1 - cols[:, ::-1].argmax(1), 0).astype(float)
    centre, half = (left + right) / 2, np.maximum((right - left) / 2, 6)
    yn = np.clip((yy - box[2]) / (box[3] - box[2]), 0, 1)
    q = (xx - centre[:, None]) / half[:, None]
    q = q + (0.06 + 0.16 * (1 - yn) ** 1.4) * np.sin(yy / 48.0 + 1.6 * q)   # lines lick sideways towards the tips
    n_lines = 9.0
    k = np.round(q * n_lines)                                          # which line this pixel belongs to
    r = np.modf(np.abs(np.sin(k * 12.9898 + 4.1) * 43758.5453))[0]     # a fixed random number per line
    cut = np.clip((yn - r * 0.42) / 0.05, 0, 1)                        # each line dies out at its own height
    blur = lambda im, rad: np.asarray(im.filter(ImageFilter.GaussianBlur(rad)), float) / 255
    depth = np.clip(blur(mask, 30), 0, 1)
    core = np.clip(blur(core_mask(), 14) * 1.6, 0, 1)
    heat = np.clip(depth ** 1.0 * (0.5 + 0.5 * yn) + 0.55 * core, 0, 1)
    width = 0.07 + 0.80 * heat
    alpha = lines_alpha(q * n_lines, width) * m * cut
    colour = LAV[None, None, :] * (1 - heat[..., None]) + CREAM[None, None, :] * heat[..., None]
    return alpha, colour


def compose(alpha, colour, base, tmask):
    art = np.dstack([colour, alpha * 255]).astype(np.uint8)
    art_im = Image.fromarray(art, "RGBA")
    halo = art_im.filter(ImageFilter.GaussianBlur(30))
    tint = Image.new("RGBA", (S, S), (*GLOW, 0))
    tint.putalpha(halo.getchannel("A").point(lambda v: int(v * 0.55)))
    out = Image.alpha_composite(Image.alpha_composite(base, tint), art_im)
    clipped = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    clipped.paste(out, (0, 0), tmask)
    return clipped.resize((512, 512), Image.LANCZOS)


def flat_c():
    """The chosen flat mark on the same tile, for comparison."""
    base, tmask = tile()
    o, c = make_flame.flatten(make_flame.OUTER), make_flame.flatten(make_flame.CORE, make_flame.CORE_SCALE, make_flame.CORE_DX, make_flame.CORE_DY)
    x0, x1 = min(p[0] for p in o), max(p[0] for p in o)
    y0, y1 = min(p[1] for p in o), max(p[1] for p in o)
    scale = 640 / (y1 - y0)
    ox, oy = S / 2 - (x0 + x1) / 2 * scale, 505 - (y0 + y1) / 2 * scale
    art = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(art)
    for pts, colour in ((o, "#6366f1"), (c, "#a5b4fc")):
        d.polygon([(x * scale + ox, y * scale + oy) for x, y in pts], fill=colour)
    out = Image.alpha_composite(base, art)
    clipped = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    clipped.paste(out, (0, 0), tmask)
    return clipped.resize((512, 512), Image.LANCZOS)


if __name__ == "__main__":
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent / "out"
    out_dir.mkdir(parents=True, exist_ok=True)
    mask, box = silhouette()
    base, tmask = tile()
    results = {"flat": flat_c(), "flow": compose(*flow(mask, box), base, tmask)}
    for n, im in results.items():
        im.save(out_dir / f"flame_{n}.png")
    sheet = Image.new("RGBA", (512 * len(results), 512 + 120), (18, 19, 22, 255))
    for k, im in enumerate(results.values()):
        sheet.alpha_composite(im, (k * 512, 0))
        x = k * 512 + 40
        for size in (96, 64, 48, 32, 24):
            sheet.alpha_composite(im.resize((size, size), Image.LANCZOS), (x, 530))
            x += size + 22
    sheet.convert("RGB").save(out_dir / "flame_sheet.png")
    print("saved", out_dir)
