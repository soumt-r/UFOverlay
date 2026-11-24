"""Recompress stored-deflate PNGs written by the Rust tests (zlib level 9).

usage: python tools/compress_png.py docs/images [more dirs or files...]
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compress_icons import recompress_png  # noqa: E402

for arg in sys.argv[1:]:
    p = Path(arg)
    for f in sorted(p.glob("*.png") if p.is_dir() else [p]):
        before = f.stat().st_size
        f.write_bytes(recompress_png(f.read_bytes()))
        print(f"{f.name}: {before} -> {f.stat().st_size} bytes")
