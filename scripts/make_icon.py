"""生成应用图标：品牌淡黄圆角方块 + 深金 X。纯标准库，无第三方依赖。

配色取自 `apps/desktop/src/theme.ts` 的淡黄原厂色：
- 底 `#F6DE8D`（`primary.main`）
- X `#3A3009`（`primary.contrast`）—— 白色 X 在淡黄上几乎看不清

用法：
    python scripts/make_icon.py icon-source.png
    cargo tauri icon icon-source.png      # 生成各尺寸 + ico/icns
"""

import struct
import sys
import zlib

SIZE = 1024
RADIUS = 200
BRAND = (0xF6, 0xDE, 0x8D, 0xFF)
WHITE = (0x3A, 0x30, 0x09, 0xFF)
CLEAR = (0, 0, 0, 0)
BAND = 58  # X 笔画半宽


def inside_rounded(x: float, y: float, size: int, radius: float) -> bool:
    if radius <= x <= size - 1 - radius:
        return 0 <= y <= size - 1
    if radius <= y <= size - 1 - radius:
        return 0 <= x <= size - 1
    for cx, cy in (
        (radius, radius),
        (size - 1 - radius, radius),
        (radius, size - 1 - radius),
        (size - 1 - radius, size - 1 - radius),
    ):
        if (x - cx) ** 2 + (y - cy) ** 2 <= radius**2:
            return True
    return False


def build_rows() -> list[bytes]:
    rows = []
    margin = 250
    for y in range(SIZE):
        row = bytearray()
        row.append(0)  # filter type 0
        for x in range(SIZE):
            if not inside_rounded(x, y, SIZE, RADIUS):
                row += bytes(CLEAR)
                continue
            inside = margin <= x <= SIZE - 1 - margin and margin <= y <= SIZE - 1 - margin
            on_x = inside and (abs((x - margin) - (y - margin)) <= BAND or
                               abs((SIZE - 1 - margin - x) - (y - margin)) <= BAND)
            row += bytes(WHITE if on_x else BRAND)
        rows.append(bytes(row))
    return rows


def main(path: str) -> None:
    raw = b"".join(build_rows())

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as f:
        f.write(png)
    print(f"written {path} ({len(png)} bytes)")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "icon.png")
