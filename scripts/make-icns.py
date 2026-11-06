#!/usr/bin/env python3
"""Builds assets/macos/icon.icns from assets/appicon.png (256 x 256) without Apple's tools.

An .icns file is a header followed by chunks, each a four-letter type and a PNG. Sizes are only
reduced from the source, never enlarged. Run it from the repository root; needs Pillow.
"""
import io
import struct
from pathlib import Path

from PIL import Image

# type, pixel size (the 2x retina types hold the doubled size).
CHUNKS = [
    ("icp4", 16), ("icp5", 32), ("icp6", 64), ("ic07", 128), ("ic08", 256),
    ("ic11", 32), ("ic12", 64), ("ic13", 256),
]

source = Image.open("assets/appicon.png").convert("RGBA")
assert source.size[0] == source.size[1] >= 256, "appicon.png must be square and at least 256 px"

body = b""
for kind, size in CHUNKS:
    png = io.BytesIO()
    source.resize((size, size), Image.LANCZOS).save(png, "PNG", optimize=True)
    data = png.getvalue()
    body += kind.encode() + struct.pack(">I", 8 + len(data)) + data

out = Path("assets/macos/icon.icns")
out.write_bytes(b"icns" + struct.pack(">I", 8 + len(body)) + body)
print(f"wrote {out} ({out.stat().st_size} bytes)")
