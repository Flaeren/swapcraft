"""The SwapCraft icon: the butterfly (one wing real, one dissolving into the cosmos) on an indigo tile, in the
shape of the ArtCraft icons (a square with round corners, a flat colour, one cream-and-ink subject).

    python tools/make_swapcraft_icon.py <the artwork, as Gabriele drew it>

The artwork comes on a black background in a slightly uneven rounded tile. This script takes the tile out of the
black, fills its edge with the tile colour, sets it on an exact square tile with our own rounded corners and
writes:
  tools/swapcraft_icon.png      the clean 1024 px master, kept so the icon can be remade without the artwork
  hub/assets/app_icon.rgba      256 px raw RGBA, the butterfly alone: the hub's header, window and taskbar icon
  hub/assets/swapcraft.ico      every size Windows asks for, embedded in the exe
  docs/img/swapcraft-icon.png   512 px, for the README and the website
Needs: pip install pillow
"""
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
MASTER = Path(__file__).resolve().parent / "swapcraft_icon.png"
SIZE = 1024
RADIUS = 0.22  # corner radius, as a share of the side (the ArtCraft tiles are about this round)


def from_artwork(path: Path) -> Image.Image:
    art = Image.open(path).convert("RGB")
    # the tile: everything but the black that reaches the picture's edges (the ink inside is dark too, so a plain
    # threshold would take it away), with the anti-aliased rim eaten away
    dark = art.convert("L").point(lambda v: 255 if v < 14 else 0)
    w, h = dark.size
    for corner in ((0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)):
        if dark.getpixel(corner) == 255:
            ImageDraw.floodfill(dark, corner, 128)
    tile = dark.point(lambda v: 0 if v == 128 else 255).filter(ImageFilter.MinFilter(9))
    box = tile.getbbox()
    # the tile colour: the median of a band just inside its left and right edges
    band = [art.getpixel((x, y)) for y in range(box[1] + 200, box[3] - 200, 7) for x in (box[0] + 8, box[2] - 9)]
    bg = tuple(sorted(c[i] for c in band)[len(band) // 2] for i in range(3))
    flat = Image.new("RGB", art.size, bg)
    art = Image.composite(art, flat, tile)
    art = art.crop(box)
    side = round(max(art.size) * 1.02)
    square = Image.new("RGB", (side, side), bg)
    square.paste(art, ((side - art.width) // 2, (side - art.height) // 2))
    return square.resize((SIZE, SIZE), Image.LANCZOS)


def rounded(img: Image.Image) -> Image.Image:
    # drawn four times larger and scaled down, for smooth corners
    big = SIZE * 4
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, big - 1, big - 1), radius=round(big * RADIUS), fill=255)
    mask = mask.resize((SIZE, SIZE), Image.LANCZOS)
    out = img.convert("RGBA")
    out.putalpha(ImageChops.multiply(mask, Image.new("L", (SIZE, SIZE), 255)))
    return out


BUTTERFLY = (85, 390, 975, 960)  # the butterfly in the 1024 px master, wing tips included


def butterfly_alone(square: Image.Image) -> Image.Image:
    """The master with everything but the butterfly painted black: moons, galaxy, orbit and the loose stars go.

    The butterfly is the cream shape that holds the body once the dust of the dissolving wing is joined to it
    (shapes grown by 4 px); an opening of 13 px first cuts the thin orbit that touches the wings, which also cuts
    the antennae, so those come back from a box around the head. Two dashes of the orbit that still touch the left
    hind wing are cleared by hand: this works on this one drawing, not on any artwork."""
    import numpy as np

    lum = np.asarray(square.convert("L"))
    ink = Image.fromarray(((lum > 40) * 255).astype("uint8"))
    grown = ink.filter(ImageFilter.MaxFilter(9))
    body = (500, 650)
    whole = grown.copy()
    ImageDraw.floodfill(whole, body, 128)
    opened = grown.filter(ImageFilter.MinFilter(13)).filter(ImageFilter.MaxFilter(13))
    ImageDraw.floodfill(opened, body, 128)
    keep = np.asarray(Image.fromarray(((np.asarray(opened) == 128) * 255).astype("uint8")).filter(ImageFilter.MaxFilter(5))) > 0
    head = np.zeros(keep.shape, bool)
    head[440:610, 420:600] = True
    keep |= head & (np.asarray(whole) == 128)
    keep[868:918, 398:485] = False
    keep[725:770, 165:200] = False
    mask = Image.fromarray((keep * 255).astype("uint8")).filter(ImageFilter.GaussianBlur(2))
    return Image.composite(square, Image.new("RGB", square.size, (0, 0, 0)), mask)


def small_master(square: Image.Image) -> Image.Image:
    """The butterfly alone on the tile, filling it, with a little more contrast: the art for the small sizes (and,
    since 10/11/2026, for the icon inside the app: header, title bar, taskbar)."""
    from PIL import ImageEnhance

    square = butterfly_alone(square)
    bg = square.getpixel((60, 520))
    art = square.crop(BUTTERFLY)
    side = round(max(art.size) * 1.2)  # room for the wing tips inside the round corners
    out = Image.new("RGB", (side, side), bg)
    out.paste(art, ((side - art.width) // 2, (side - art.height) // 2))
    return ImageEnhance.Contrast(out.resize((SIZE, SIZE), Image.LANCZOS)).enhance(1.25)


def main() -> None:
    if len(sys.argv) > 1:
        square = from_artwork(Path(sys.argv[1]))
        square.save(MASTER)
    else:
        square = Image.open(MASTER).convert("RGB")
    icon = rounded(square)
    small = rounded(small_master(square))
    assets = ROOT / "hub" / "assets"
    # inside the app the icon is shown small (header, title bar, taskbar): the moons and the galaxy only blur, so
    # it carries the butterfly alone (Gabriele, 10/11/2026); the full picture stays for 64 px and up in the .ico
    (assets / "app_icon.rgba").write_bytes(small.resize((256, 256), Image.LANCZOS).tobytes())
    # Up to 48 px (Start, the taskbar, File Explorer's lists) the moons and the galaxy turn into noise: those sizes
    # show the butterfly alone, larger. The ICO writer takes, for each size, the given image of exactly that size.
    big_sizes = [64, 128, 256]
    small_sizes = [16, 20, 24, 32, 40, 48]
    frames = [small.resize((s, s), Image.LANCZOS) for s in small_sizes] + [icon.resize((s, s), Image.LANCZOS) for s in big_sizes]
    frames[-1].save(assets / "swapcraft.ico", sizes=[(s, s) for s in small_sizes + big_sizes], append_images=frames[:-1])
    icon.resize((512, 512), Image.LANCZOS).save(ROOT.parent / "docs" / "img" / "swapcraft-icon.png")
    print("saved", MASTER.name, "app_icon.rgba", "swapcraft.ico", "swapcraft-icon.png")


if __name__ == "__main__":
    main()
