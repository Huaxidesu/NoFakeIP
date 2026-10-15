#!/usr/bin/env python3
"""校验 APK 内打包的启动图标与本地生成的图标是否一致（逐像素比对）。

为什么要单独校验：Android 会**重命名并混淆**资源（APK 里只有 `res/o-.png`
这种短名），所以没法靠文件名确认图标换没换；而且图标是"看不出问题"的那类东西
—— 构建成功、应用能跑，但可能还是默认图标。这里直接把两个 PNG 解成像素比。

做法：拿 `gen/android/.../mipmap-*/ic_launcher.png`（构建时真正被打包的那份）
作为参照，在 APK 的 `res/` 里找出同尺寸的方形 PNG，逐像素比较。

用法：
    python tools/verify_apk_icon.py [apk路径]
"""

import glob
import os
import shutil
import struct
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_APK = os.path.join(ROOT, "output", "android", "NoFakeIP_0.1.0_arm64.apk")
MIPMAP_ROOT = os.path.join(
    ROOT, "desktop", "src-tauri", "gen", "android", "app", "src", "main", "res"
)


def png_size(path):
    with open(path, "rb") as f:
        head = f.read(24)
    if head[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    return struct.unpack(">II", head[16:24])


def to_rgba(path):
    out = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
        capture_output=True,
    )
    if out.returncode != 0:
        return None
    return out.stdout


def main():
    apk = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_APK
    if not os.path.exists(apk):
        raise SystemExit(f"找不到 APK：{apk}")

    refs = {}
    for d in sorted(glob.glob(os.path.join(MIPMAP_ROOT, "mipmap-*dpi"))):
        p = os.path.join(d, "ic_launcher.png")
        if os.path.exists(p):
            size = png_size(p)
            if size:
                refs[size[0]] = p
    if not refs:
        raise SystemExit("找不到参照图标：先执行 python tools/make_icons.py --android-only")
    print(f"参照图标尺寸：{sorted(refs)}")

    tmp = tempfile.mkdtemp(prefix="apkicon-")
    try:
        subprocess.run(["unzip", "-o", "-q", apk, "res/*.png", "-d", tmp], check=True)
        candidates = []
        for p in glob.glob(os.path.join(tmp, "res", "*.png")):
            size = png_size(p)
            if size and size[0] == size[1] and size[0] in refs:
                candidates.append(p)
        print(f"APK 内同尺寸方形 PNG：{len(candidates)} 个")

        matched = set()
        for p in candidates:
            size = png_size(p)[0]
            if size in matched:
                continue
            a, b = to_rgba(p), to_rgba(refs[size])
            if a is None or b is None or a != b:
                continue
            matched.add(size)
            print(f"  {size}x{size}: ✅ 与 {os.path.relpath(refs[size], ROOT)} 逐像素一致")

        missing = sorted(set(refs) - matched)
        print()
        if missing:
            print(f"❌ 以下尺寸未在 APK 内找到一致图标：{missing}")
            raise SystemExit(1)
        print(f"✅ {len(matched)}/{len(refs)} 个密度的启动图标全部匹配，APK 图标已替换")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
