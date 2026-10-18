"""Write a small RGBA test PNG (stdlib only). Usage: make_png.py OUT [WIDTH HEIGHT]"""
import struct, sys, zlib

w, h = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (64, 48)
rows = b"".join(
    b"\x00" + b"".join(bytes((x * 4, y * 5, 128, 255)) for x in range(w)) for y in range(h)
)

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
