#!/usr/bin/env python3
"""Rebuild `icon.icns` so Relay sits at the same size as every other app in the Dock.

macOS does not draw an app icon edge to edge. Apple's icon grid puts the rounded tile
in 824 of a 1024 canvas — a little over 80% — with the remaining 100px on each side
left transparent, and every system and well-built third-party icon obeys it. The Dock
then lays icons out on their *canvas*, not on their artwork, so a full-bleed icon is
drawn about 24% wider than its neighbours and looks oversized next to them. That is
what Relay's was: a 1024x1024 tile with no padding at all.

Only the `.icns` is rebuilt, and deliberately so. The PNGs beside it are a different
job:

  - `128x128@2x.png` is the in-app mark. `RelayMark.tsx` imports it straight from this
    directory and draws it flush at 22px in the title bar, so padding baked into the
    file would show up as a mark that had mysteriously shrunk inside its own box.
  - `icon.ico` and the `Square*Logo.png` files are Windows, where full-bleed is the
    convention and this padding would be wrong.

So the artwork stays full-bleed everywhere except the one file macOS reads.

The source is the 1024 image already inside the existing `.icns`, which is the largest
copy of the artwork in the repository — there is no vector original. Corner radius is
left alone: measured at 0.219 of the width against Apple's 0.225, close enough that
rounding it further would be a redrawing rather than a fix.

Requires Pillow and macOS's `iconutil`:

    python3 scripts/macos-icon.py
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

ICONS = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
ICNS = ICONS / "icon.icns"

CANVAS = 1024
#: Apple's macOS icon grid. The tile is 824 of 1024, centred.
TILE = 824

#: What `iconutil` expects in an `.iconset`: (filename, pixel size).
SIZES = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]


def largest(icns: Path, into: Path) -> Image.Image:
    """The biggest image inside an `.icns`, as RGBA."""
    subprocess.run(
        ["iconutil", "-c", "iconset", str(icns), "-o", str(into)],
        check=True,
        capture_output=True,
    )
    biggest = max(into.glob("*.png"), key=lambda p: Image.open(p).size[0])
    return Image.open(biggest).convert("RGBA")


def pad(art: Image.Image) -> Image.Image:
    """The artwork scaled onto Apple's grid, centred on a transparent canvas."""
    tile = art.resize((TILE, TILE), Image.LANCZOS)
    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    canvas.paste(tile, ((CANVAS - TILE) // 2, (CANVAS - TILE) // 2), tile)
    return canvas


def main() -> int:
    if not ICNS.exists():
        print(f"no icon at {ICNS}", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        art = largest(ICNS, work / "current.iconset")
        if art.size != (CANVAS, CANVAS):
            art = art.resize((CANVAS, CANVAS), Image.LANCZOS)

        fill = art.getbbox()
        already = (fill[2] - fill[0]) / CANVAS
        print(f"source artwork fills {already:.1%} of its canvas (target {TILE / CANVAS:.1%})")

        master = pad(art)
        out = work / "Relay.iconset"
        out.mkdir()
        for name, size in SIZES:
            master.resize((size, size), Image.LANCZOS).save(out / name)

        built = work / "icon.icns"
        subprocess.run(
            ["iconutil", "-c", "icns", str(out), "-o", str(built)],
            check=True,
            capture_output=True,
        )
        shutil.copy(built, ICNS)

    print(f"wrote {ICNS} ({ICNS.stat().st_size:,} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
