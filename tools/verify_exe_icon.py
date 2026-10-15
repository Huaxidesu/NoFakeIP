#!/usr/bin/env python3
"""校验 PE 文件（.exe）内嵌的图标是否与给定 ICO 一致。

为什么不用 PowerShell：本机安全策略禁止 `Add-Type` 在运行时编译 .NET 代码，
`ExtractAssociatedIcon` 用不了。这里直接解析 PE 资源目录，把 RT_ICON 里
每个尺寸的像素解出来，和 ICO 里对应尺寸逐像素比较，比"看图标没变"可靠。

用法：
    python tools/verify_exe_icon.py <exe> [ico]
"""

import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_ICO = os.path.join(ROOT, "desktop", "src-tauri", "icons", "icon.ico")

RT_ICON = 3


def read_pe_sections(data):
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e_lfanew : e_lfanew + 4] != b"PE\x00\x00":
        raise SystemExit("不是 PE 文件")
    coff = e_lfanew + 4
    nsec, = struct.unpack_from("<H", data, coff + 2)
    opt_size, = struct.unpack_from("<H", data, coff + 16)
    opt = coff + 20
    magic, = struct.unpack_from("<H", data, opt)
    dd_off = opt + (112 if magic == 0x20B else 96)
    nsec_off = opt + opt_size

    secs = []
    for i in range(nsec):
        o = nsec_off + 40 * i
        vsize, vaddr, rsize, raddr = struct.unpack_from("<IIII", data, o + 8)
        secs.append((vaddr, max(vsize, rsize), raddr))
    res_rva, _ = struct.unpack_from("<II", data, dd_off + 2 * 8)
    return secs, res_rva


def rva2off(secs, rva):
    for vaddr, size, raddr in secs:
        if vaddr <= rva < vaddr + size:
            return raddr + (rva - vaddr)
    return None


def read_dir(data, base, off):
    """返回 [(id, 子目录偏移 or None, 数据项偏移 or None)]。"""
    n_named, n_id = struct.unpack_from("<HH", data, off + 12)
    out = []
    for i in range(n_named + n_id):
        e = off + 16 + 8 * i
        name_id, offset = struct.unpack_from("<II", data, e)
        if offset & 0x80000000:
            out.append((name_id & 0xFFFF, offset & 0x7FFFFFFF, None))
        else:
            out.append((name_id & 0xFFFF, None, offset))
    return out


def extract_icons(data):
    """返回 {资源ID: bytes}，即 RT_ICON 里每个尺寸的原始数据。"""
    secs, res_rva = read_pe_sections(data)
    base = rva2off(secs, res_rva)
    if base is None:
        raise SystemExit("找不到资源节")
    icons = {}
    for tid, sub, _ in read_dir(data, base, base):
        if tid != RT_ICON or sub is None:
            continue
        for rid, rsub, _ in read_dir(data, base, base + sub):
            if rsub is None:
                continue
            for _, _, dentry in read_dir(data, base, base + rsub):
                if dentry is None:
                    continue
                off = base + dentry
                # IMAGE_RESOURCE_DATA_ENTRY：OffsetToData(RVA) 在前，Size 在后
                rva, size = struct.unpack_from("<II", data, off)
                doff = rva2off(secs, rva)
                if doff is None:
                    continue
                icons[rid] = data[doff : doff + size]
    return icons


def decode_to_rgba(payload):
    """把 RT_ICON 数据解成 (w, h, rgba bytes)。支持 PNG 与 32 位 DIB。"""
    if payload[:4] == b"\x89PNG":
        import subprocess
        import tempfile

        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
            f.write(payload)
            tmp = f.name
        try:
            raw = subprocess.run(
                ["ffmpeg", "-v", "error", "-i", tmp, "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                capture_output=True,
            )
            if raw.returncode != 0:
                return None
            w = struct.unpack_from(">I", payload, 16)[0]
            return w, w, raw.stdout
        finally:
            os.unlink(tmp)

    # DIB：BITMAPINFOHEADER + 调色板 + XOR 位图 + AND 掩码
    hdr_size, w, h2, planes, bpp = struct.unpack_from("<IiiHH", payload, 0)
    if bpp != 32:
        return None
    h = abs(h2) // 2  # ICO 的 DIB 高度是 2 倍（XOR + AND）
    pix_off = hdr_size
    rgba = bytearray(w * h * 4)
    for y in range(h):
        src_y = h - 1 - y  # DIB 自下而上
        for x in range(w):
            o = pix_off + (src_y * w + x) * 4
            b, g, r, a = payload[o : o + 4]
            d = (y * w + x) * 4
            rgba[d : d + 4] = bytes((r, g, b, a))
    return w, h, bytes(rgba)


def read_ico(path):
    blob = open(path, "rb").read()
    _, _, count = struct.unpack("<HHH", blob[:6])
    out = []
    for i in range(count):
        off = 6 + 16 * i
        w, h, _, _, _, _, size, dataoff = struct.unpack("<BBBBHHII", blob[off : off + 16])
        out.append((0 if w == 0 else w, blob[dataoff : dataoff + size]))
    return out


def main():
    exe = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "output", "windows", "NoFakeIP.exe")
    ico = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_ICO

    data = open(exe, "rb").read()
    icons = extract_icons(data)
    print(f"{os.path.basename(exe)} 内 RT_ICON 资源：{len(icons)} 个（资源ID: {sorted(icons)}）")

    ico_entries = read_ico(ico)
    print(f"ICO 内尺寸：{[s for s, _ in ico_entries]}")

    matched = 0
    for size, payload in ico_entries:
        dec = decode_to_rgba(payload)
        if dec is None:
            print(f"  {size}x{size}: ICO 条目无法解码，跳过")
            continue
        w, h, want = dec
        hit = None
        for rid, blob in icons.items():
            got = decode_to_rgba(blob)
            if got is None or got[0] != w or got[1] != h:
                continue
            if got[2] == want:
                hit = rid
                break
        if hit is not None:
            matched += 1
            print(f"  {size}x{size}: ✅ 与 exe 内资源 #{hit} 逐像素一致")
        else:
            print(f"  {size}x{size}: ❌ exe 内没有一致的图标")

    print()
    if matched == len(ico_entries):
        print(f"✅ {matched}/{len(ico_entries)} 个尺寸全部匹配，exe 图标已替换")
    else:
        print(f"❌ 仅 {matched}/{len(ico_entries)} 个尺寸匹配")
        raise SystemExit(1)


if __name__ == "__main__":
    main()
