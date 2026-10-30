"""The hub's own icon: the flame (variant C) alone, large, on a transparent square (no tile around it).

    python tools/make_hub_icon.py

Writes hub/assets/app_icon.rgba (raw RGBA, for the window and taskbar icon) and hub/assets/voidflare.ico.
"""
from pathlib import Path

from PIL import Image

import make_flame

SIZE = 256
FILL = 0.94   # how much of the height the flame takes

assets = Path(__file__).resolve().parent.parent / "hub" / "assets"
outer, core = make_flame.LOOKS["DARK"]            # the brighter indigo reads on dark and light taskbars alike
flame = make_flame.render(outer, core, round(SIZE * FILL))
icon = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
icon.alpha_composite(flame, ((SIZE - flame.width) // 2, (SIZE - flame.height) // 2))
(assets / "app_icon.rgba").write_bytes(icon.tobytes())
icon.save(assets / "voidflare.ico", sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)])
print("saved", assets)
