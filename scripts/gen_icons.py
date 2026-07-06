#!/usr/bin/env python3
"""Generate the app + tray icons (pure stdlib: zlib + struct PNG writer).

App icon: rounded-square gradient with a white waveform glyph.
Tray icons: alpha-only glyphs (macOS template images) — idle/listening/processing.
"""
import struct, zlib, os, math

OUT = os.path.join(os.path.dirname(__file__), "..", "apps", "desktop", "src-tauri", "icons")


def write_png(path, w, h, rgba_rows):
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(row) for row in rgba_rows)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as f:
        f.write(png)


def rounded_mask(x, y, w, h, r):
    cx = min(max(x, r), w - r)
    cy = min(max(y, r), h - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r or (r <= x < w - r) or (r <= y < h - r)


def bars(size):
    """Waveform bar geometry: (center_x_frac, half_height_frac) per bar."""
    return [(0.22, 0.16), (0.36, 0.30), (0.50, 0.42), (0.64, 0.30), (0.78, 0.16)]


def app_icon(size):
    rows = []
    r = size * 0.22
    bw = max(2, int(size * 0.075))
    glyph = bars(size)
    for y in range(size):
        row = []
        for x in range(size):
            if rounded_mask(x, y, size, size, r):
                t = (x + y) / (2 * size)
                pr, pg, pb = int(30 + 40 * t), int(60 + 60 * t), int(180 + 60 * t)
                a = 255
                for cxf, hf in glyph:
                    cx = cxf * size
                    hh = hf * size
                    if abs(x - cx) <= bw / 2 and abs(y - size / 2) <= hh:
                        pr, pg, pb = 255, 255, 255
                        break
                row += [pr, pg, pb, a]
            else:
                row += [0, 0, 0, 0]
        rows.append(row)
    return rows


def tray_icon(size, mode):
    rows = []
    bw = max(2, int(size * 0.11))
    glyph = bars(size)
    for y in range(size):
        row = []
        for x in range(size):
            a = 0
            if mode == "processing":
                # three dots
                for cxf in (0.25, 0.5, 0.75):
                    cx, cy = cxf * size, size / 2
                    if (x - cx) ** 2 + (y - cy) ** 2 <= (size * 0.09) ** 2:
                        a = 255
            else:
                scale = 1.0 if mode == "listening" else 0.55
                for cxf, hf in glyph:
                    cx = cxf * size
                    hh = hf * size * scale
                    if abs(x - cx) <= bw / 2 and abs(y - size / 2) <= hh:
                        a = 255
                        break
            row += [0, 0, 0, a]
        rows.append(row)
    return rows


os.makedirs(OUT, exist_ok=True)
for name, size in (("32x32.png", 32), ("128x128.png", 128), ("icon.png", 256)):
    write_png(os.path.join(OUT, name), size, size, app_icon(size))
for mode in ("idle", "listening", "processing"):
    write_png(os.path.join(OUT, f"tray-{mode}.png"), 32, 32, tray_icon(32, mode))
print("icons written to", os.path.abspath(OUT))
