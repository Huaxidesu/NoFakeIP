#!/usr/bin/env python3
"""生成 NoFakeIP 应用图标（PNG + ICO）。

不依赖任何第三方库（只用标准库 zlib/struct），保证在干净环境下也能重跑。
图案：蓝紫渐变圆角方块 + 白色字母 N（用矩形拼出，避免依赖字体）。

用法：
    python tools/gen_icons.py
输出到 desktop/src-tauri/icons/
"""

import os
import struct
import zlib

OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "desktop", "src-tauri", "icons")

# 与前端主色保持一致（doc/08-GUI规格.md 第 5 节）
C_FROM = (0x3B, 0x82, 0xF6)  # --accent
C_TO = (0x8B, 0x5C, 0xF6)  # --secondary
C_FG = (0xFF, 0xFF, 0xFF)

SIZES = [16, 32, 48, 64, 128, 256, 512]


def lerp(a, b, t):
    return int(round(a + (b - a) * t))


def inside_rounded_rect(x, y, size, radius):
    """圆角矩形判定（含抗锯齿的近似：返回覆盖度 0..1）。"""
    r = radius
    # 距离最近圆角圆心
    cx = min(max(x, r), size - 1 - r)
    cy = min(max(y, r), size - 1 - r)
    dx = x - cx
    dy = y - cy
    d = (dx * dx + dy * dy) ** 0.5
    if d <= r - 1:
        return 1.0
    if d >= r + 0.5:
        return 0.0
    return max(0.0, min(1.0, (r + 0.5 - d) / 1.5))


def in_n_letter(x, y, size):
    """用三个矩形拼出字母 N（相对尺寸，便于任意分辨率）。"""
    m = size * 0.30  # 外边距
    w = size * 0.115  # 笔画宽度
    top = m
    bot = size - m
    left = m
    right = size - m

    # 左竖
    if left <= x <= left + w and top <= y <= bot:
        return True
    # 右竖
    if right - w <= x <= right and top <= y <= bot:
        return True
    # 斜杠：从左下到右上
    t = (y - top) / (bot - top)
    cx = left + (right - left) * t
    if abs(x - cx) <= w * 0.62 and top <= y <= bot:
        return True
    return False


def render(size):
    """返回 RGBA bytes。"""
    px = bytearray(size * size * 4)
    radius = size * 0.22
    for y in range(size):
        t = y / max(1, size - 1)
        bg = (lerp(C_FROM[0], C_TO[0], t), lerp(C_FROM[1], C_TO[1], t), lerp(C_FROM[2], C_TO[2], t))
        for x in range(size):
            cover = inside_rounded_rect(x, y, size, radius)
            if cover <= 0:
                continue
            if in_n_letter(x, y, size):
                r, g, b = C_FG
            else:
                r, g, b = bg
            a = int(255 * cover)
            i = (y * size + x) * 4
            px[i] = r
            px[i + 1] = g
            px[i + 2] = b
            px[i + 3] = a
    return bytes(px)


def write_png(path, size, rgba):
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # filter type 0
        raw.extend(rgba[y * stride : (y + 1) * stride])

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        c += struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        return c

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def write_ico(path, entries):
    """entries: [(size, png_bytes)] —— 使用 PNG 压缩的 ICO（Vista+ 支持）。"""
    count = len(entries)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    directory = b""
    data = b""
    for size, png in entries:
        w = 0 if size >= 256 else size
        h = 0 if size >= 256 else size
        directory += struct.pack(
            "<BBBBHHII", w, h, 0, 0, 1, 32, len(png), offset
        )
        offset += len(png)
        data += png
    with open(path, "wb") as f:
        f.write(header + directory + data)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    rendered = {}
    for s in SIZES:
        rendered[s] = render(s)

    targets = {
        32: "32x32.png",
        128: "128x128.png",
        256: "128x128@2x.png",
        512: "icon.png",
    }
    for size, fname in targets.items():
        p = os.path.join(OUT_DIR, fname)
        write_png(p, size, rendered[size])
        print(f"  -> {fname} ({size}x{size})")

    # ICO 打包多个尺寸，保证任务栏 / 资源管理器都清晰
    ico_sizes = [16, 32, 48, 64, 128, 256]
    ico_entries = []
    for s in ico_sizes:
        # 复用已渲染的大图缩略：小尺寸单独渲染保证清晰
        ico_entries.append((s, png_bytes(s)))
    write_ico(os.path.join(OUT_DIR, "icon.ico"), ico_entries)
    print("  -> icon.ico")

    # 额外：StoreLogo 与 Square 系列（Tauri Windows 打包可能引用）
    for name, size in [
        ("Square30x30Logo.png", 30),
        ("Square44x44Logo.png", 44),
        ("Square71x71Logo.png", 71),
        ("Square89x89Logo.png", 89),
        ("Square107x107Logo.png", 107),
        ("Square142x142Logo.png", 142),
        ("Square150x150Logo.png", 150),
        ("Square284x284Logo.png", 284),
        ("Square310x310Logo.png", 310),
        ("StoreLogo.png", 50),
    ]:
        p = os.path.join(OUT_DIR, name)
        write_png(p, size, render(size))
    print("  -> Windows Store logo 系列")


def png_bytes(size):
    """在内存里生成指定尺寸的 PNG 字节。"""
    rgba = render(size)
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)
        raw.extend(rgba[y * stride : (y + 1) * stride])

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        c += struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        return c

    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    out += chunk(b"IEND", b"")
    return out


if __name__ == "__main__":
    main()
