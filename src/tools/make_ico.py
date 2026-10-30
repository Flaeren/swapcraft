"""Makes each app's Windows icon from its engraved icon: `<app>/app.ico` (16 to 256 px, for the exe)
and `<app>/icon.py` (64 px and 40 px PNGs as base64: the window icon and the in-app header).

    python tools/make_ico.py

Run `engrave_icons.py` first. Only apps that already have a folder with an app.py are done.
"""
import base64
import io
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SIZES = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)]

for source in sorted((ROOT / "hub" / "assets" / "icons").glob("*.png")):
    big = Image.open(source).convert("RGBA")
    rust = ROOT / "crates" / f"app-{source.stem}"
    if (rust / "Cargo.toml").is_file():
        # Rust apps: raw RGBA for the window (256) and the header (128), and the exe icon.
        (rust / "assets").mkdir(exist_ok=True)
        for size in (256, 128):
            (rust / "assets" / f"icon_{size}.rgba").write_bytes(big.resize((size, size), Image.LANCZOS).tobytes())
        big.resize((256, 256), Image.LANCZOS).save(rust / "assets" / "app.ico", sizes=SIZES)
        print("rust icon for", rust.name)
    app = ROOT.parent / "legacy" / source.stem
    if not (app / "app.py").is_file():
        continue
    big.resize((256, 256), Image.LANCZOS).save(app / "app.ico", sizes=SIZES)
    def b64(size):
        buf = io.BytesIO()
        big.resize((size, size), Image.LANCZOS).save(buf, "PNG", optimize=True)
        return base64.b64encode(buf.getvalue()).decode()

    (app / "icon.py").write_text(
        '"""The app\'s own icon as base64 PNGs: PNG (64 px, the window icon) and HEADER (40 px, shown in the\n'
        'app\'s header). Made by tools/make_ico.py, do not edit."""\n'
        f'PNG = "{b64(64)}"\n'
        f'HEADER = "{b64(40)}"\n',
        encoding="utf-8", newline="\n")
    print("icon for", app.name)
