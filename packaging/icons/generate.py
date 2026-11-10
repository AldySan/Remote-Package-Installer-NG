#!/usr/bin/env python3
"""Regenerates every icon format shipped with Remote Package Installer NG.

The single source of truth is ``crates/rpi-ng/assets/icon.png`` (1024x1024,
RGBA, transparent corners). This script derives every other format from it:

- ``crates/rpi-ng/assets/icon_256.png`` — runtime window icon decoded by the
  app itself (title bar / taskbar on Windows and X11).
- ``crates/rpi-ng/assets/icon.ico`` — multi-size Win32 icon resource embedded
  into the ``.exe`` by ``crates/rpi-ng/build.rs`` (Explorer / taskbar /
  jumplist before the window exists).
- ``crates/rpi-ng/assets/AppIcon.icns`` — macOS bundle icon referenced by
  ``packaging/macos/Info.plist`` (``CFBundleIconFile``).
- ``packaging/linux/icons/hicolor/<size>/apps/remote-package-installer-ng.png``
  — freedesktop icon-theme sizes for the ``.desktop`` file.

Run from anywhere inside the repo::

    python3 packaging/icons/generate.py

Requires Pillow (``pip install pillow``). With ``--redraw`` the 1024 master is
redrawn from the vector description below instead of being reused, so
tweaking colors/geometry only means editing this file.
"""

from __future__ import annotations

import io
import struct
import sys
from pathlib import Path

from PIL import Image, ImageDraw

REPO = Path(__file__).resolve().parents[2]
ASSETS = REPO / "crates" / "rpi-ng" / "assets"
HICOLOR = REPO / "packaging" / "linux" / "icons" / "hicolor"

MASTER_SIZE = 1024  # edge length of the master PNG
SS = 4  # supersampling factor: draw at 4096, downscale with Lanczos

# Brand palette: deep PS-blue gradient, white install glyph.
BG_TOP = (18, 57, 107)  # #12396B
BG_BOTTOM = (31, 111, 191)  # #1F6FBF
EDGE = (12, 39, 75)  # #0C274B, subtle inner stroke for light backgrounds
WHITE = (255, 255, 255, 255)

ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
ICNS_CHUNKS = [
    ("ic11", 32),
    ("ic12", 64),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
]
HICOLOR_SIZES = [48, 64, 128, 256]

APP_NAME = "remote-package-installer-ng"  # binary, app_id and icon key


def redraw_master() -> Image.Image:
    """Draws the 1024 master: rounded square + download-into-console glyph."""
    size = MASTER_SIZE * SS
    cx = size / 2

    # Vertical gradient clipped to a rounded square (transparent corners).
    column = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        column.putpixel(
            (0, y),
            tuple(round(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM)),
        )
    gradient = column.resize((size, size))

    corner_mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(corner_mask).rounded_rectangle(
        [0, 0, size - 1, size - 1], radius=int(size * 0.22), fill=255
    )
    icon = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    icon.paste(gradient, (0, 0), corner_mask)

    # Subtle inner stroke so the icon keeps an edge on light backgrounds.
    stroke = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    inset = int(size * 0.006)
    ImageDraw.Draw(stroke).rounded_rectangle(
        [inset, inset, size - 1 - inset, size - 1 - inset],
        radius=int(size * 0.214),
        outline=EDGE + (110,),
        width=max(1, int(size * 0.004)),
    )
    icon = Image.alpha_composite(icon, stroke)

    # White glyph: a download arrow above a console base bar, one polygon
    # each (no seams), sized to stay readable down to 16 px.
    glyph = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(glyph)
    shaft_half = 0.075 * size  # arrow stem half-width
    head_half = 0.28 * size  # arrowhead half-width
    top, mid, tip = 0.20 * size, 0.50 * size, 0.70 * size
    draw.polygon(
        [
            (cx - shaft_half, top),
            (cx + shaft_half, top),
            (cx + shaft_half, mid),
            (cx + head_half, mid),
            (cx, tip),
            (cx - head_half, mid),
            (cx - shaft_half, mid),
        ],
        fill=WHITE,
    )
    bar_half = 0.29 * size
    draw.rounded_rectangle(
        [cx - bar_half, 0.775 * size, cx + bar_half, 0.875 * size],
        radius=int(0.05 * size),
        fill=WHITE,
    )
    icon = Image.alpha_composite(icon, glyph)

    return icon.resize((MASTER_SIZE, MASTER_SIZE), Image.LANCZOS)


def png_bytes(frame: Image.Image) -> bytes:
    buf = io.BytesIO()
    frame.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def build_icns(master: Image.Image, path: Path) -> None:
    """Minimal modern icns: one PNG chunk per type (ic07..ic10, ic11, ic12)."""
    chunks = bytearray()
    for chunk_type, px in ICNS_CHUNKS:
        frame = master.resize((px, px), Image.LANCZOS)
        png = png_bytes(frame)
        chunks += chunk_type.encode("ascii")
        chunks += struct.pack(">I", 8 + len(png))
        chunks += png
    blob = b"icns" + struct.pack(">I", 8 + len(chunks)) + bytes(chunks)
    path.write_bytes(blob)


def main() -> int:
    ASSETS.mkdir(parents=True, exist_ok=True)
    master_path = ASSETS / "icon.png"
    if "--redraw" in sys.argv or not master_path.exists():
        master = redraw_master()
        master.save(master_path, format="PNG", optimize=True)
        print(f"wrote {master_path} ({master.width}x{master.height})")
    else:
        master = Image.open(master_path).convert("RGBA")
        if master.size != (MASTER_SIZE, MASTER_SIZE):
            print(
                f"error: {master_path} is {master.width}x{master.height}, "
                f"expected {MASTER_SIZE}x{MASTER_SIZE} — rerun with --redraw"
            )
            return 1
        print(f"reusing {master_path} ({master.width}x{master.height})")

    # Runtime window icon (embedded via include_bytes!, decoded with `image`).
    runtime = master.resize((256, 256), Image.LANCZOS)
    runtime.save(ASSETS / "icon_256.png", format="PNG", optimize=True)
    print(f"wrote {ASSETS / 'icon_256.png'} (256x256)")

    # Win32 multi-size icon resource.
    master.save(
        ASSETS / "icon.ico",
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
    )
    print(f"wrote {ASSETS / 'icon.ico'} (sizes {ICO_SIZES})")

    # macOS bundle icon.
    build_icns(master, ASSETS / "AppIcon.icns")
    print(f"wrote {ASSETS / 'AppIcon.icns'} (chunks {[t for t, _ in ICNS_CHUNKS]})")

    # Linux freedesktop icon-theme sizes.
    for px in HICOLOR_SIZES:
        dest = HICOLOR / f"{px}x{px}" / "apps" / f"{APP_NAME}.png"
        dest.parent.mkdir(parents=True, exist_ok=True)
        master.resize((px, px), Image.LANCZOS).save(dest, format="PNG", optimize=True)
        print(f"wrote {dest} ({px}x{px})")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
