"""Procedural "engraved" cosmic app icons: cream hairlines on a dark tile, thickness following the light,
in the manner of old astronomical plates. Deterministic (fixed seeds), so the icons can be regenerated.

    python tools/engrave_icons.py            # writes hub/assets/icons/<app-id>.png (512 px) and .rgba (128 px)
    python tools/engrave_icons.py OUT_DIR    # a preview sheet and the PNGs in OUT_DIR instead

One icon per app, in the app's colour (the same colours as hub/src/catalog.rs):
  dwg-to-pdf constellation, word-to-pdf comet, excel-to-pdf ringed planet, slides-to-pdf eclipse,
  pdf-tools binary star, image-converter nebula, audio-converter pulsar, video-converter black hole.
"""
import math
import random
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

S = 1024
CREAM = (243, 234, 214)


# ---------------------------------------------------------------- drawing helpers
def tile_base(color):
    inner = tuple(int(0.16 * c + 0.84 * b) for c, b in zip(color, (26, 26, 50)))
    outer = tuple(int(0.05 * c + 0.95 * b) for c, b in zip(color, (9, 9, 18)))
    yy, xx = np.mgrid[0:S, 0:S]
    t = np.clip(np.hypot(xx - S * 0.5, yy - S * 0.4) / (S * 0.75), 0, 1)[..., None]
    rgb = (np.array(inner) * (1 - t) + np.array(outer) * t).astype(np.uint8)
    im = Image.fromarray(np.dstack([rgb, np.full((S, S), 255, np.uint8)]), "RGBA")
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([S * .04, S * .04, S * .96, S * .96], radius=S * .22, fill=255)
    return im, mask


def ribbon(d, pts, widths, fill):
    """A line of varying width: polygon through the left and right offsets of `pts`."""
    if len(pts) < 2:
        return
    left, right = [], []
    for i, (x, y) in enumerate(pts):
        x0, y0 = pts[max(i - 1, 0)]
        x1, y1 = pts[min(i + 1, len(pts) - 1)]
        tx, ty = x1 - x0, y1 - y0
        n = math.hypot(tx, ty) or 1.0
        nx, ny = -ty / n, tx / n
        h = widths[i] / 2
        left.append((x + nx * h, y + ny * h))
        right.append((x - nx * h, y - ny * h))
    d.polygon(left + right[::-1], fill=fill)


def lines_alpha(phase, width, feather=1.0):
    """Anti-aliased lines at integer values of `phase`; `width` is the line width as a fraction of the spacing."""
    gy, gx = np.gradient(phase)
    grad = np.maximum(np.hypot(gx, gy), 0.004)
    dist_px = np.abs(phase - np.round(phase)) / grad
    return np.clip((width / grad / 2 - dist_px) / feather + 0.5, 0, 1)


def glow_blob(d, x, y, radii_alpha, colour=(255, 244, 214)):
    for rad, a in radii_alpha:
        d.ellipse([x - rad, y - rad, x + rad, y + rad], fill=(*colour, a))


def star4(d, x, y, length, colour=(255, 244, 214), alpha=210, width=2):
    for dx, dy, ln in ((1, 0, length), (0, 1, length), (.7, .7, length * .5), (.7, -.7, length * .5)):
        d.line([(x - dx * ln, y - dy * ln), (x + dx * ln, y + dy * ln)], fill=(*colour, alpha), width=width)


def sphere(d, cx, cy, r, tilt, roll, light, lines, gamma, k, ink, samples=520):
    """A globe drawn as latitude lines (seen from `tilt`), line width following the lighting."""
    lx, ly, lz = light
    ln = math.sqrt(lx * lx + ly * ly + lz * lz)
    lx, ly, lz = lx / ln, ly / ln, lz / ln
    spacing = math.pi * r / lines
    ct, st, cr, sr = math.cos(tilt), math.sin(tilt), math.cos(roll), math.sin(roll)
    for i in range(lines):
        phi = -math.pi / 2 + math.pi * (i + .5) / lines
        seg, wid = [], []
        for j in range(samples + 1):
            lam = -math.pi + 2 * math.pi * j / samples
            x, y, z = math.cos(phi) * math.cos(lam), math.sin(phi), math.cos(phi) * math.sin(lam)
            y, z = y * ct - z * st, y * st + z * ct
            x, y = x * cr - y * sr, x * sr + y * cr
            if z > 0.03:
                b = 0.06 + 0.94 * max(0.0, x * lx + y * ly + z * lz)
                seg.append((cx + r * x, cy - r * y))
                wid.append(max(0.0, min(spacing * 0.95, spacing * k * b ** gamma)))
            else:
                ribbon(d, seg, wid, ink)
                seg, wid = [], []
        ribbon(d, seg, wid, ink)


# ---------------------------------------------------------------- the eight subjects
def saturn(art, color):
    d = ImageDraw.Draw(art, "RGBA")
    cx, cy, r = 512, 520, 205
    ring_tilt, ring_rot = 0.30, math.radians(-17)
    radii = [r * (1.28 + 0.035 * i) for i in range(20) if not (9 <= i <= 10)]

    def ring(front):
        for n, a in enumerate(radii):
            pts, wid = [], []
            for j in range(0, 361):
                th = math.radians(j)
                if (math.sin(th) > 0) != front:
                    continue
                x, y = a * math.cos(th), a * ring_tilt * math.sin(th)
                px = cx + x * math.cos(ring_rot) - y * math.sin(ring_rot)
                py = cy + x * math.sin(ring_rot) + y * math.cos(ring_rot)
                if not front and math.hypot(px - cx, py - cy) < r:
                    continue
                pts.append((px, py))
                wid.append(4.2 * (1.0 - 0.4 * n / len(radii)))
            ribbon(d, pts, wid, (*CREAM, 235))
    ring(False)
    sphere(d, cx, cy, r, tilt=0.42, roll=math.radians(-17), light=(-0.55, 0.55, 0.62), lines=30, gamma=1.25, k=0.80,
           ink=(*CREAM, 255))
    ring(True)
    return [(512, 520, 470)]


def eclipse(art, color):
    d = ImageDraw.Draw(art, "RGBA")
    rnd = random.Random(7)
    cx, cy, R = 512, 520, 176
    for i in range(520):
        th = 2 * math.pi * i / 520 + rnd.uniform(-.004, .004)
        streamer = 0.42 + 0.58 * abs(math.cos(2 * (th - math.radians(-12)))) ** 1.6
        length = R * (0.16 + 1.15 * rnd.random() ** 2.4 * streamer)
        bow = rnd.uniform(-.05, .05) * length
        pts, wid = [], []
        for s in range(15):
            u = s / 14
            rr = R * 1.015 + length * u
            ang = th + bow / max(rr, 1) * math.sin(u * math.pi)
            pts.append((cx + rr * math.cos(ang), cy + rr * math.sin(ang)))
            wid.append(max(0.4, 3.4 - 3.0 * u + 0.5))
        ribbon(d, pts, wid, (*CREAM, int(235 - 150 * min(1, length / (R * 1.1)))))
    for k, rr in enumerate((1.02, 1.04, 1.065)):
        d.ellipse([cx - R * rr, cy - R * rr, cx + R * rr, cy + R * rr], outline=(*CREAM, 240 - 50 * k), width=3 - (k > 1))
    ang = math.radians(-48)
    px, py = cx + R * 1.02 * math.cos(ang), cy + R * 1.02 * math.sin(ang)
    glow_blob(d, px, py, ((46, 40), (30, 70), (16, 130), (7, 255)))
    star4(d, px, py, 70)
    return [(512, 520, 470)]


def comet(art, color):
    d = ImageDraw.Draw(art, "RGBA")
    rnd = random.Random(11)
    hx, hy, r = 690, 330, 38
    direction = math.radians(136)
    for _ in range(230):
        off = rnd.uniform(-1, 1) * r * 1.05
        a = direction + rnd.uniform(-1, 1) * math.radians(15) * (rnd.random() ** 0.6)
        length = 160 + 470 * rnd.random() ** 1.6
        bow = rnd.uniform(-.09, .09)
        sx, sy = hx + off * math.cos(a + math.pi / 2), hy + off * math.sin(a + math.pi / 2)
        pts, wid = [], []
        for s in range(17):
            u = s / 16
            ang = a + bow * u * u
            pts.append((sx + length * u * math.cos(ang), sy + length * u * math.sin(ang)))
            wid.append(max(0.5, 6.2 * (1 - u) ** 1.1 * (1.2 - abs(off) / (r * 1.2)) + 0.5))
        ribbon(d, pts, wid, (*CREAM, int(225 - 130 * min(1, length / 630))))
    glow_blob(d, hx, hy, ((90, 26), (62, 50), (46, 90)), (215, 232, 255))
    d.ellipse([hx - r, hy - r, hx + r, hy + r], fill=(250, 252, 255, 255))
    star4(d, hx, hy, 96, (235, 244, 255), 170, 3)
    return [(690, 330, 160), (500, 560, 300)]


def constellation(art, color):
    """A figure of stars joined by double hairlines, with a dimension line under it."""
    d = ImageDraw.Draw(art, "RGBA")
    rnd = random.Random(5)
    P = [(222, 690), (330, 500), (528, 420), (722, 330), (806, 560), (580, 610), (470, 770)]
    links = [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 2), (5, 6), (6, 0), (1, 5)]
    for a, b in links:
        (x0, y0), (x1, y1) = P[a], P[b]
        dx, dy = x1 - x0, y1 - y0
        n = math.hypot(dx, dy)
        ux, uy, nx, ny = dx / n, dy / n, -dy / n, dx / n
        for off in (-4.0, 4.0):
            pts = [(x0 + ux * (30 + t * (n - 60)) + nx * off, y0 + uy * (30 + t * (n - 60)) + ny * off) for t in np.linspace(0, 1, 24)]
            ribbon(d, pts, [2.0 + 2.2 * math.sin(math.pi * i / 23) for i in range(24)], (*CREAM, 225))
    for k, (x, y) in enumerate(P):
        big = k in (3, 2)
        rr = 15 if big else 11
        glow_blob(d, x, y, ((rr * 3.2, 30), (rr * 1.9, 80)), (230, 255, 248))
        d.ellipse([x - rr, y - rr, x + rr, y + rr], fill=(*CREAM, 255))
        star4(d, x, y, 58 if big else 36, (230, 255, 248), 200, 3)
    # the dimension line: end ticks and an arrowhead each side
    yy = 880
    d.line([(222, yy), (806, yy)], fill=(*CREAM, 160), width=3)
    for x, s in ((222, 1), (806, -1)):
        d.line([(x, yy - 22), (x, yy + 22)], fill=(*CREAM, 190), width=3)
        d.polygon([(x, yy), (x + s * 26, yy - 9), (x + s * 26, yy + 9)], fill=(*CREAM, 190))
    return [(512, 560, 520)]


def binary(art, color):
    """Two stars with hairline coronas, a stream of plasma between them and thin orbits."""
    d = ImageDraw.Draw(art, "RGBA")
    rnd = random.Random(9)
    A, B = (385, 575, 64), (665, 450, 38)
    tint = tuple(min(255, int(0.45 * c + 140)) for c in color)
    for (x, y, r), ink in ((A, CREAM), (B, tint)):
        for i in range(260):
            th = 2 * math.pi * i / 260 + rnd.uniform(-.01, .01)
            length = r * (0.28 + 0.9 * rnd.random() ** 1.8)
            pts = [(x + (r * 1.05 + length * u) * math.cos(th), y + (r * 1.05 + length * u) * math.sin(th)) for u in np.linspace(0, 1, 9)]
            ribbon(d, pts, [max(0.4, 3.0 * (1 - u) + 0.4) for u in np.linspace(0, 1, 9)], (*ink, int(230 - 120 * length / (r * 1.2))))
    # stream of plasma from the big star to the small one: a spindle of hairlines that never cross
    dx, dy = B[0] - A[0], B[1] - A[1]
    dist = math.hypot(dx, dy)
    nx, ny = -dy / dist, dx / dist
    for k in range(25):
        off = (k - 12) * 6.2
        pts = []
        for t in np.linspace(0.06, 0.94, 40):
            bulge = off * math.sin(math.pi * t) ** 0.9
            pts.append((A[0] + dx * t + nx * bulge, A[1] + dy * t + ny * bulge))
        ribbon(d, pts, [0.9 + 2.2 * math.sin(math.pi * i / 39) ** 0.7 for i in range(40)], (*CREAM, 135))
    # the orbit: three thin ellipses, broken
    mx, my = (A[0] + B[0]) / 2, (A[1] + B[1]) / 2
    rot = math.atan2(B[1] - A[1], B[0] - A[0])
    for a_r, b_r in ((380, 190), (345, 165), (310, 140)):
        pts = []
        for j in range(0, 361, 3):
            th = math.radians(j)
            x, y = a_r * math.cos(th), b_r * math.sin(th)
            pts.append((mx + x * math.cos(rot) - y * math.sin(rot), my + x * math.sin(rot) + y * math.cos(rot)))
        for s in range(0, len(pts) - 8, 18):
            ribbon(d, pts[s:s + 11], [2.4] * 11, (*CREAM, 110))
    for (x, y, r), ink in ((A, CREAM), (B, tint)):
        glow_blob(d, x, y, ((r * 1.9, 30), (r * 1.4, 60), (r * 1.1, 120)), ink)
        d.ellipse([x - r, y - r, x + r, y + r], fill=(*ink, 255))
    return [(512, 520, 480)]


def nebula(art, color):
    """A cloud engraved with diagonal hatching whose thickness follows the density."""
    rng = np.random.default_rng(4)

    def noise(cells):
        small = rng.random((cells, cells)).astype(np.float32)
        return np.asarray(Image.fromarray((small * 255).astype(np.uint8)).resize((S, S), Image.BICUBIC), float) / 255

    fbm = sum(noise(c) * a for c, a in ((4, 1.0), (9, .6), (18, .35), (36, .2))) / 2.15
    yy, xx = np.mgrid[0:S, 0:S].astype(float)
    ang = math.radians(-28)
    u = (xx - 512) * math.cos(ang) + (yy - 520) * math.sin(ang)
    v = -(xx - 512) * math.sin(ang) + (yy - 520) * math.cos(ang)
    env = np.exp(-(u / 265) ** 2 - (v / 165) ** 2)
    density = np.clip((fbm * 1.9 - 0.80) * env * 2.6, 0, 1)
    warp = 1.4 * noise(7)

    def hatch(angle, spacing, power):
        phase = (xx * math.cos(math.radians(angle)) + yy * math.sin(math.radians(angle))) / spacing + warp
        return lines_alpha(phase, 0.10 + 0.85 * density ** power)

    alpha = np.maximum(hatch(38, 8.5, 0.9), hatch(88, 8.5, 1.8) * (density > 0.45)) * np.clip(density * 7, 0, 1)
    t = np.clip(density ** 1.4, 0, 1)[..., None]
    lav = np.array([int(0.5 * c + 128) for c in color], float)
    colour = lav * (1 - t) + np.array(CREAM, float) * t
    art.alpha_composite(Image.fromarray(np.dstack([colour, alpha * 235]).astype(np.uint8), "RGBA"))
    d = ImageDraw.Draw(art, "RGBA")
    rnd = random.Random(2)
    for _ in range(26):
        x, y = rnd.randint(250, 780), rnd.randint(260, 780)
        if density[y, x] > 0.25:
            glow_blob(d, x, y, ((15, 55), (7, 255)))
            star4(d, x, y, 30, alpha=170)
    return [(512, 520, 330)]


def pulsar(art, color):
    """A neutron star with two beams of hairlines and thin rings of pulses."""
    d = ImageDraw.Draw(art, "RGBA")
    rnd = random.Random(13)
    cx, cy = 512, 520
    axis = math.radians(-32)
    for sign in (1, -1):
        for _ in range(150):
            a = axis + (math.pi if sign < 0 else 0) + rnd.uniform(-1, 1) * math.radians(9) * rnd.random() ** 0.5
            length = 330 * (0.45 + 0.55 * rnd.random())
            pts = [(cx + (40 + length * u) * math.cos(a), cy + (40 + length * u) * math.sin(a)) for u in np.linspace(0, 1, 14)]
            ribbon(d, pts, [max(0.5, 5.4 * (1 - u) ** 0.9 + 0.5) for u in np.linspace(0, 1, 14)], (*CREAM, int(235 - 120 * length / 330)))
    for k, rr in enumerate((95, 150, 215, 290, 375)):
        for s in range(8):  # arcs, with the beam directions left open
            a0 = math.radians(45 * s + 8 + 3 * k)
            if min(abs((a0 - axis + math.pi) % (2 * math.pi) - math.pi), abs((a0 - axis) % (2 * math.pi) - math.pi)) < math.radians(24):
                continue
            pts = [(cx + rr * math.cos(a0 + math.radians(j)), cy + rr * math.sin(a0 + math.radians(j))) for j in range(0, 29, 2)]
            ribbon(d, pts, [max(1.2, 3.8 - 0.55 * k)] * len(pts), (*CREAM, int(215 - 28 * k)))
    glow_blob(d, cx, cy, ((74, 40), (52, 80), (34, 150)))
    d.ellipse([cx - 24, cy - 24, cx + 24, cy + 24], fill=(255, 250, 240, 255))
    return [(512, 520, 480)]


def blackhole(art, color):
    """A dark disc with a photon ring, a lensed accretion disk and the arcs of light bent over and under it."""
    d = ImageDraw.Draw(art, "RGBA")
    cx, cy, R = 512, 512, 98
    tilt, rot = 0.21, math.radians(-12)
    n = 36
    hot = tuple(min(255, int(0.5 * c + 160)) for c in color)
    for i in range(n):
        a = R * (1.62 + 0.056 * i)
        t = i / (n - 1)
        ink = tuple(int(CREAM[k] * (1 - t) ** 0.7 + hot[k] * (1 - (1 - t) ** 0.7)) for k in range(3))
        pts = []
        for j in range(0, 361, 2):
            th = math.radians(j)
            x, y = a * math.cos(th), a * tilt * math.sin(th)
            px = cx + x * math.cos(rot) - y * math.sin(rot)
            py = cy + x * math.sin(rot) + y * math.cos(rot)
            if math.sin(th) < 0 and math.hypot(px - cx, py - cy) < R * 1.03:
                pts.append(None)
            else:
                pts.append((px, py))
        run = []
        for p in pts + [None]:
            if p is None:
                if len(run) > 1:
                    ribbon(d, run, [4.6 - 2.6 * t] * len(run), (*ink, int(245 - 90 * t)))
                run = []
            else:
                run.append(p)
    # the light bent over the top (many arcs) and under the bottom (a few)
    for k in range(12):
        rr = R * (1.10 + 0.045 * k)
        arc = [(cx + rr * math.cos(math.radians(j)), cy + rr * math.sin(math.radians(j)) * 1.0) for j in range(198 - k, 343 + k, 2)]
        ribbon(d, arc, [3.8 - 0.22 * k] * len(arc), (*CREAM, int(235 - 12 * k)))
    for k in range(5):
        rr = R * (1.10 + 0.05 * k)
        arc = [(cx + rr * math.cos(math.radians(j)), cy + rr * math.sin(math.radians(j))) for j in range(30 + 4 * k, 150 - 4 * k, 2)]
        ribbon(d, arc, [3.2 - 0.4 * k] * len(arc), (*CREAM, int(215 - 28 * k)))
    d.ellipse([cx - R * 1.04, cy - R * 1.04, cx + R * 1.04, cy + R * 1.04], outline=(255, 250, 235, 255), width=7)
    return [(512, 512, 470)]


def galaxy(art, color):
    """A spiral galaxy seen face-on: two logarithmic arms of hairlines around a bright core (a book of pages)."""
    d = ImageDraw.Draw(art, "RGBA")
    cx, cy = 512, 512
    for arm in range(2):
        base = arm * math.pi
        for strand in range(9):
            pts, wid = [], []
            spread = (strand - 4) * 0.055
            for k in range(0, 241):
                t = k / 240.0
                th = base + spread * (0.4 + t) + 4.6 * t
                rr = 52 + 300 * t ** 0.92
                pts.append((cx + rr * math.cos(th), cy + rr * 0.86 * math.sin(th)))
                wid.append(max(0.4, (5.2 - 4.4 * t) * (1 - abs(strand - 4) / 6.0)))
            ribbon(d, pts, wid, (*CREAM, int(235 - 14 * abs(strand - 4))))
    glow_blob(d, cx, cy, [(120, 14), (84, 26), (54, 60), (30, 150), (14, 255)])
    star4(d, cx, cy, 70, alpha=190, width=3)
    return [(512, 512, 345)]


# app id -> (painter, the app's colour, a seed for the star field)
ICONS = {
    "ebook-converter": (galaxy, (232, 176, 90)),
    "dwg-to-pdf": (constellation, (63, 214, 184)),
    "word-to-pdf": (comet, (45, 129, 255)),
    "excel-to-pdf": (saturn, (74, 210, 122)),
    "slides-to-pdf": (eclipse, (255, 193, 77)),
    "pdf-tools": (binary, (255, 98, 89)),
    "image-converter": (nebula, (176, 140, 255)),
    "audio-converter": (pulsar, (255, 122, 200)),
    "video-converter": (blackhole, (77, 200, 255)),
}


def star_field(art, rnd, avoid):
    d = ImageDraw.Draw(art, "RGBA")
    for _ in range(90):
        x, y = rnd.uniform(70, S - 70), rnd.uniform(70, S - 70)
        if any(math.hypot(x - ax, y - ay) < ar for ax, ay, ar in avoid):
            continue
        rr = rnd.choice((1.6, 1.8, 2.2, 2.6, 3.4))
        d.ellipse([x - rr, y - rr, x + rr, y + rr], fill=(*CREAM, rnd.randint(110, 235)))


def render(app_id):
    painter, color = ICONS[app_id]
    base, mask = tile_base(color)
    art = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    avoid = painter(art, color)
    star_field(art, random.Random(3), avoid)
    halo = art.filter(ImageFilter.GaussianBlur(26))
    tint = Image.new("RGBA", (S, S), (*color, 0))
    tint.putalpha(halo.getchannel("A").point(lambda v: int(v * 0.30)))
    out = Image.alpha_composite(Image.alpha_composite(base, tint), art)
    clipped = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    clipped.paste(out, (0, 0), mask)
    return clipped.resize((512, 512), Image.LANCZOS)


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    preview = len(sys.argv) > 1
    out_dir = Path(sys.argv[1]) if preview else root / "hub" / "assets" / "icons"
    out_dir.mkdir(parents=True, exist_ok=True)
    icons = {n: render(n) for n in ICONS}
    for n, im in icons.items():
        im.save(out_dir / f"{n}.png")
        if not preview:
            (out_dir / f"{n}.rgba").write_bytes(im.resize((128, 128), Image.LANCZOS).tobytes())
    if not preview:
        print("saved", out_dir)
        raise SystemExit
    cols = 4
    sheet = Image.new("RGBA", (400 * cols, 400 * 2 + 2 * 70), (18, 19, 22, 255))
    for k, (n, im) in enumerate(icons.items()):
        x, y = (k % cols) * 400, (k // cols) * 470
        sheet.alpha_composite(im.resize((384, 384), Image.LANCZOS), (x + 8, y + 6))
        sx = x + 12
        for size in (56, 40, 28, 20):
            sheet.alpha_composite(im.resize((size, size), Image.LANCZOS), (sx, y + 400))
            sx += size + 16
    sheet.convert("RGB").save(out_dir / "sheet.png")
    print("saved", out_dir)
