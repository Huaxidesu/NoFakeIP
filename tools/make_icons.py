#!/usr/bin/env python3
"""NoFakeIP 图标生成工具（只依赖标准库 + ffmpeg 解码 JPEG）。

为什么需要它：应用图标要在多个平台保持一致 —— Windows 的 exe/ICO、
应用内品牌标识、Android 各密度 mipmap。手工维护这些尺寸既容易漏又容易不一致，
所以统一由本脚本从一张源图生成。

图案来源：用户提供的白底 JPG（蓝紫圆角方块 + 白色字母 N）。
JPG 没有透明通道，脚本会把**外部白色画布**抠掉（从四边泛洪填充，
这样方块内部的白色字母 N 不会被误伤），输出带透明通道的方形图标。

用法：
    # 1) 由原始 JPG 生成规范源图（只需在换图标时执行一次）
    python tools/make_icons.py --from-jpg <原始图.jpg>

    # 2) 由规范源图生成各平台图标
    python tools/make_icons.py

    # 3) 只更新 Android（tools/build-android.sh 会调用，gen/ 重建后自动补回）
    python tools/make_icons.py --android-only
"""

import argparse
import os
import struct
import subprocess
import sys
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SOURCE_PNG = os.path.join(ROOT, "tools", "icon_source.png")
TAURI_ICONS = os.path.join(ROOT, "desktop", "src-tauri", "icons")
APP_ASSETS = os.path.join(ROOT, "desktop", "src", "assets")
ANDROID_RES = os.path.join(
    ROOT, "desktop", "src-tauri", "gen", "android", "app", "src", "main", "res"
)

FFMPEG = os.environ.get("FFMPEG", "ffmpeg")

# 判定「背景白」的阈值：足够亮且几乎没有色偏
BG_MIN_LUMA = 232
BG_MAX_CHROMA = 20


# —— 解码 / 编码 ——


def decode_image(path):
    """用 ffmpeg 解码任意图片为 RGB bytes，返回 (w, h, data)。"""
    probe = subprocess.run(
        [
            os.environ.get("FFPROBE", "ffprobe"),
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=p=0",
            path,
        ],
        capture_output=True,
        text=True,
    )
    if probe.returncode != 0:
        raise SystemExit(f"ffprobe 读取失败：{probe.stderr.strip()}")
    w, h = (int(x) for x in probe.stdout.strip().split(",")[:2])

    raw = subprocess.run(
        [FFMPEG, "-v", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
        capture_output=True,
    )
    if raw.returncode != 0:
        raise SystemExit(f"ffmpeg 解码失败：{raw.stderr.decode('utf-8', 'ignore').strip()}")
    data = raw.stdout
    if len(data) != w * h * 3:
        raise SystemExit(f"解码尺寸不符：期望 {w*h*3} 字节，实际 {len(data)}")
    return w, h, data


def read_png_rgba(path):
    """读取本脚本自己写出的 PNG（8 位 RGBA、无隔行），返回 (w, h, bytearray)。"""
    with open(path, "rb") as f:
        blob = f.read()
    if blob[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"不是 PNG：{path}")
    pos = 8
    w = h = None
    idat = bytearray()
    while pos < len(blob):
        (length,) = struct.unpack(">I", blob[pos : pos + 4])
        tag = blob[pos + 4 : pos + 8]
        body = blob[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if tag == b"IHDR":
            w, h, depth, color, comp, filt, interlace = struct.unpack(">IIBBBBB", body)
            if depth != 8 or color != 6 or interlace != 0:
                raise SystemExit("只支持 8 位 RGBA、非隔行的 PNG")
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break

    raw = zlib.decompress(bytes(idat))
    stride = w * 4
    out = bytearray(w * h * 4)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ftype = raw[p]
        p += 1
        line = bytearray(raw[p : p + stride])
        p += stride
        if ftype == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 0xFF
        elif ftype == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b = prev[i]
                c = prev[i - 4] if i >= 4 else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        out[y * stride : (y + 1) * stride] = line
        prev = line
    return w, h, out


def _png_chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def encode_png(w, h, rgba):
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)
        raw.extend(rgba[y * stride : (y + 1) * stride])
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + _png_chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + _png_chunk(b"IEND", b"")
    )


def write_png(path, w, h, rgba):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(encode_png(w, h, rgba))


def write_ico(path, entries):
    """entries: [(size, png_bytes)] —— 用 PNG 压缩的 ICO（Vista+ 支持）。"""
    count = len(entries)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    directory = b""
    data = b""
    for size, png in entries:
        dim = 0 if size >= 256 else size
        directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
        data += png
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(header + directory + data)


# —— 抠图 ——


def build_alpha(w, h, rgb):
    """返回 alpha 掩码（0..255）。

    做法：从四条边对「背景白」像素做泛洪填充，填充到的算外部（透明），
    其余算图标内部（不透明）。这样方块内部的白色字母 N 会被正确保留。
    """
    n = w * h
    is_bg = bytearray(n)
    for i in range(n):
        o = i * 3
        r, g, b = rgb[o], rgb[o + 1], rgb[o + 2]
        mx, mn = max(r, g, b), min(r, g, b)
        luma = (r * 299 + g * 587 + b * 114) // 1000
        if luma >= BG_MIN_LUMA and (mx - mn) <= BG_MAX_CHROMA:
            is_bg[i] = 1

    outside = bytearray(n)
    stack = []
    for x in range(w):
        for y in (0, h - 1):
            i = y * w + x
            if is_bg[i] and not outside[i]:
                outside[i] = 1
                stack.append(i)
    for y in range(h):
        for x in (0, w - 1):
            i = y * w + x
            if is_bg[i] and not outside[i]:
                outside[i] = 1
                stack.append(i)

    while stack:
        i = stack.pop()
        x, y = i % w, i // w
        if x > 0:
            j = i - 1
            if is_bg[j] and not outside[j]:
                outside[j] = 1
                stack.append(j)
        if x + 1 < w:
            j = i + 1
            if is_bg[j] and not outside[j]:
                outside[j] = 1
                stack.append(j)
        if y > 0:
            j = i - w
            if is_bg[j] and not outside[j]:
                outside[j] = 1
                stack.append(j)
        if y + 1 < h:
            j = i + w
            if is_bg[j] and not outside[j]:
                outside[j] = 1
                stack.append(j)

    return bytes(0 if outside[i] else 255 for i in range(n))


def crop_to_content(w, h, rgb, alpha):
    minx, miny, maxx, maxy = w, h, -1, -1
    for y in range(h):
        row = y * w
        for x in range(w):
            if alpha[row + x]:
                if x < minx:
                    minx = x
                if x > maxx:
                    maxx = x
                if y < miny:
                    miny = y
                if y > maxy:
                    maxy = y
    if maxx < 0:
        raise SystemExit("没有找到图标内容（整张图都被判为背景白）")

    cw, ch = maxx - minx + 1, maxy - miny + 1
    out_rgb = bytearray(cw * ch * 3)
    out_a = bytearray(cw * ch)
    for y in range(ch):
        src = ((y + miny) * w + minx) * 3
        dst = y * cw * 3
        out_rgb[dst : dst + cw * 3] = rgb[src : src + cw * 3]
        src_a = (y + miny) * w + minx
        out_a[y * cw : (y + 1) * cw] = alpha[src_a : src_a + cw]
    return cw, ch, out_rgb, out_a


def square_pad(w, h, rgb, alpha):
    """裁成正方形（以较长边为准，居中），保证后续缩放不变形。"""
    side = max(w, h)
    ox, oy = (side - w) // 2, (side - h) // 2
    out_rgb = bytearray(side * side * 3)
    out_a = bytearray(side * side)
    for y in range(h):
        dst = ((y + oy) * side + ox) * 3
        out_rgb[dst : dst + w * 3] = rgb[y * w * 3 : (y + 1) * w * 3]
        out_a[(y + oy) * side + ox : (y + oy) * side + ox + w] = alpha[y * w : (y + 1) * w]
    return side, side, out_rgb, out_a


# —— 缩放 ——


def resize_rgba(w, h, rgb, alpha, tw, th):
    """盒式降采样（预乘 alpha），边缘自然带抗锯齿。"""
    out = bytearray(tw * th * 4)
    for ty in range(th):
        y0 = ty * h // th
        y1 = max(y0 + 1, (ty + 1) * h // th)
        for tx in range(tw):
            x0 = tx * w // tw
            x1 = max(x0 + 1, (tx + 1) * w // tw)
            sa = sr = sg = sb = 0
            count = 0
            for y in range(y0, y1):
                base = y * w
                for x in range(x0, x1):
                    i = base + x
                    a = alpha[i]
                    count += 1
                    if a:
                        o = i * 3
                        sa += a
                        sr += rgb[o] * a
                        sg += rgb[o + 1] * a
                        sb += rgb[o + 2] * a
            o = (ty * tw + tx) * 4
            if sa:
                out[o] = sr // sa
                out[o + 1] = sg // sa
                out[o + 2] = sb // sa
                out[o + 3] = sa // count
            else:
                out[o + 3] = 0
    return bytes(out)


def make_circular(rgba, size):
    """把方形图标裁成圆形（Android 的 ic_launcher_round）。"""
    out = bytearray(rgba)
    r = size / 2.0
    for y in range(size):
        dy = y + 0.5 - r
        for x in range(size):
            dx = x + 0.5 - r
            d = (dx * dx + dy * dy) ** 0.5
            o = (y * size + x) * 4 + 3
            if d >= r:
                out[o] = 0
            elif d > r - 1:
                out[o] = int(out[o] * (r - d))
    return bytes(out)


def make_adaptive_foreground(src_rgba, src_size, canvas, content_ratio=0.667):
    """Android 自适应图标前景：canvas 见方，内容缩到中间 content_ratio 的安全区内。

    规范要求前景是 108dp 画布、内容落在中心 72dp 内（72/108 ≈ 0.667），
    否则被启动器按各种形状裁切时会切到图案。
    """
    inner = max(1, int(round(canvas * content_ratio)))
    small = resize_rgba(
        src_size, src_size,
        _rgb_from_rgba(src_rgba), _alpha_from_rgba(src_rgba),
        inner, inner,
    )
    out = bytearray(canvas * canvas * 4)
    off = (canvas - inner) // 2
    for y in range(inner):
        dst = ((y + off) * canvas + off) * 4
        out[dst : dst + inner * 4] = small[y * inner * 4 : (y + 1) * inner * 4]
    return bytes(out)


def _rgb_from_rgba(rgba):
    n = len(rgba) // 4
    out = bytearray(n * 3)
    for i in range(n):
        out[i * 3 : i * 3 + 3] = rgba[i * 4 : i * 4 + 3]
    return bytes(out)


def _alpha_from_rgba(rgba):
    n = len(rgba) // 4
    return bytes(rgba[i * 4 + 3] for i in range(n))


# —— 各平台输出 ——


def build_source_from_jpg(jpg_path):
    print(f"读取原始图：{jpg_path}")
    w, h, rgb = decode_image(jpg_path)
    print(f"  尺寸 {w}x{h}")
    alpha = build_alpha(w, h, rgb)
    kept = sum(1 for a in alpha if a)
    print(f"  抠掉外部白底后保留 {kept / (w * h) * 100:.1f}% 像素")
    cw, ch, crgb, ca = crop_to_content(w, h, rgb, alpha)
    print(f"  内容包围盒 {cw}x{ch}")
    sw, sh, srgb, sa = square_pad(cw, ch, crgb, ca)
    size = 1024
    rgba = resize_rgba(sw, sh, srgb, sa, size, size)
    write_png(SOURCE_PNG, size, size, rgba)
    print(f"  -> {os.path.relpath(SOURCE_PNG, ROOT)} ({size}x{size})")


def load_source():
    if not os.path.exists(SOURCE_PNG):
        raise SystemExit(
            f"缺少 {os.path.relpath(SOURCE_PNG, ROOT)}；"
            "请先执行 python tools/make_icons.py --from-jpg <原始图.jpg>"
        )
    w, h, rgba = read_png_rgba(SOURCE_PNG)
    if w != h:
        raise SystemExit("源图必须是正方形")
    return w, rgba


def gen_desktop(src_rgba, src_size):
    rgb = _rgb_from_rgba(src_rgba)
    alpha = _alpha_from_rgba(src_rgba)

    def at(size):
        return resize_rgba(src_size, src_size, rgb, alpha, size, size)

    print("桌面端图标：")
    for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png"), (512, "icon.png")]:
        write_png(os.path.join(TAURI_ICONS, name), size, size, at(size))
        print(f"  -> icons/{name} ({size}x{size})")

    entries = [(s, encode_png(s, s, at(s))) for s in (16, 32, 48, 64, 128, 256)]
    write_ico(os.path.join(TAURI_ICONS, "icon.ico"), entries)
    print("  -> icons/icon.ico (16/32/48/64/128/256)")

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
        write_png(os.path.join(TAURI_ICONS, name), size, size, at(size))
    print("  -> Windows Store logo 系列")

    logo = at(192)
    write_png(os.path.join(APP_ASSETS, "logo.png"), 192, 192, logo)
    print("  -> src/assets/logo.png (192x192，应用内品牌标识)")


def gen_android(src_rgba, src_size):
    if not os.path.isdir(ANDROID_RES):
        print("跳过 Android：gen/android 尚未生成（先执行 pnpm tauri android init）")
        return
    rgb = _rgb_from_rgba(src_rgba)
    alpha = _alpha_from_rgba(src_rgba)

    # Android 各密度的启动图标尺寸
    densities = {
        "mdpi": 48,
        "hdpi": 72,
        "xhdpi": 96,
        "xxhdpi": 144,
        "xxxhdpi": 192,
    }
    print("Android 启动图标：")
    for dpi, size in densities.items():
        icon = resize_rgba(src_size, src_size, rgb, alpha, size, size)
        d = os.path.join(ANDROID_RES, f"mipmap-{dpi}")
        write_png(os.path.join(d, "ic_launcher.png"), size, size, icon)
        write_png(os.path.join(d, "ic_launcher_round.png"), size, size, make_circular(icon, size))
        # 自适应图标前景：108dp 画布，内容占中间 66%（安全区）
        fg_size = int(round(size * 108 / 48))
        fg = make_adaptive_foreground(src_rgba, src_size, fg_size)
        write_png(os.path.join(d, "ic_launcher_foreground.png"), fg_size, fg_size, fg)
        print(f"  -> mipmap-{dpi}/ic_launcher[_round/_foreground].png ({size}/{fg_size}px)")


def main():
    ap = argparse.ArgumentParser(description="NoFakeIP 图标生成")
    ap.add_argument("--from-jpg", metavar="PATH", help="由原始 JPG 生成 tools/icon_source.png")
    ap.add_argument("--android-only", action="store_true", help="只更新 Android mipmap")
    ap.add_argument("--desktop-only", action="store_true", help="只更新桌面端图标")
    args = ap.parse_args()

    if args.from_jpg:
        build_source_from_jpg(args.from_jpg)
        if not (args.android_only or args.desktop_only):
            print()

    size, rgba = load_source()
    if not args.android_only:
        gen_desktop(rgba, size)
    if not args.desktop_only:
        gen_android(rgba, size)
    print("\n完成。")


if __name__ == "__main__":
    main()
