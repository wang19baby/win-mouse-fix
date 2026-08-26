#!/usr/bin/env python3
"""Generate assets/icon.ico: a classic up-left cursor arrow (white fill,
black outline, alpha) at 16/32/48 px. Pure stdlib, no PIL.

The ICO is embedded into the binary via include_bytes! in src/win/tray.rs and
turned into an HICON at runtime (no Windows resource toolchain needed).
"""
import struct
import os

# Classic up-left cursor arrow (y-down), in a 0..32 box.
ARROW = [
    (2, 1), (2, 18), (9, 18), (6, 23),
    (16, 30), (19, 28), (10, 21), (17, 21),
    (14, 16), (2, 16),
]


def point_in_poly(x, y, poly):
    inside = False
    n = len(poly)
    j = n - 1
    for i in range(n):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if ((yi > y) != (yj > y)) and (x < (xj - xi) * (y - yi) / (yj - yi) + xi):
            inside = not inside
        j = i
    return inside


def draw(size):
    poly = [(x * size / 32.0, y * size / 32.0) for (x, y) in ARROW]
    rows = []
    for y in range(size):
        row = []
        for x in range(size):
            inside = point_in_poly(x + 0.5, y + 0.5, poly)
            if not inside:
                row.append((0, 0, 0, 0))
                continue
            edge = False
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx, ny = x + dx, y + dy
                if nx < 0 or ny < 0 or nx >= size or ny >= size or not point_in_poly(
                    nx + 0.5, ny + 0.5, poly
                ):
                    edge = True
                    break
            row.append((0, 0, 0, 255) if edge else (245, 245, 245, 255))
        rows.append(row)
    return rows


def make_ico(sizes):
    images = []
    for s in sizes:
        rows = draw(s)
        xor = bytearray()
        for y in range(s - 1, -1, -1):  # bottom-up
            for x in range(s):
                r, g, b, a = rows[y][x]
                xor += bytes((b, g, r, a))  # BGRA
        mask_row = (s + 31) // 32 * 4
        and_mask = b"\x00" * (mask_row * s)
        images.append((s, bytes(xor), and_mask))

    count = len(images)
    out = bytearray()
    out += struct.pack("<HHH", 0, 1, count)  # ICONDIR
    offset = 6 + count * 16
    for s, xor, and_mask in images:
        img = xor + and_mask
        out += struct.pack(
            "<BBBBHHII",
            s if s < 256 else 0,
            s if s < 256 else 0,
            0, 0, 1, 32,
            len(img),
            offset,
        )
        offset += len(img)
    for s, xor, and_mask in images:
        out += xor + and_mask
    return bytes(out)


os.makedirs("assets", exist_ok=True)
data = make_ico([16, 32, 48])
with open("assets/icon.ico", "wb") as f:
    f.write(data)
print("wrote assets/icon.ico", len(data), "bytes")
