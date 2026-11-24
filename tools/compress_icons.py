"""Recompress the PNGs that `cargo test make_icons -- --ignored` writes.

The Rust side only emits stored (uncompressed) deflate to stay dependency-free;
zlib level 9 shrinks the icon ~10x before it gets embedded in the exe.

usage: python tools/compress_icons.py
"""
import struct
import zlib
from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"


def recompress_png(data: bytes) -> bytes:
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos, chunks, idat = 8, [], b""
    while pos < len(data):
        (n,) = struct.unpack(">I", data[pos:pos + 4])
        ty = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + n]
        pos += 12 + n
        if ty == b"IDAT":
            idat += body
        else:
            chunks.append((ty, body))
    raw = zlib.decompress(idat)
    out = bytearray(data[:8])

    def put(ty, body):
        out.extend(struct.pack(">I", len(body)) + ty + body)
        out.extend(struct.pack(">I", zlib.crc32(ty + body) & 0xFFFFFFFF))

    for ty, body in chunks:
        if ty == b"IEND":
            put(b"IDAT", zlib.compress(raw, 9))
        put(ty, body)
    return bytes(out)


def recompress_ico(data: bytes) -> bytes:
    _, _, count = struct.unpack("<HHH", data[:6])
    entries, images = [], []
    for i in range(count):
        e = data[6 + 16 * i: 22 + 16 * i]
        w, h, c, r, planes, bpp, size, off = struct.unpack("<BBBBHHII", e)
        images.append(recompress_png(data[off:off + size]))
        entries.append((w, h, c, r, planes, bpp))
    out = bytearray(struct.pack("<HHH", 0, 1, count))
    off = 6 + 16 * count
    for (w, h, c, r, planes, bpp), img in zip(entries, images):
        out += struct.pack("<BBBBHHII", w, h, c, r, planes, bpp, len(img), off)
        off += len(img)
    for img in images:
        out += img
    return bytes(out)


if __name__ == "__main__":
    for name, fn in (("ufoverlay.ico", recompress_ico), ("ufoverlay.png", recompress_png)):
        p = ASSETS / name
        before = p.stat().st_size
        p.write_bytes(fn(p.read_bytes()))
        print(f"{name}: {before} -> {p.stat().st_size} bytes")
