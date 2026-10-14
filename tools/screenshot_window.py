#!/usr/bin/env python3
"""按进程名截取窗口截图（仅用标准库：ctypes + zlib 手写 PNG）。

用途：验证 Tauri 桌面应用的真实渲染结果，不依赖第三方截图工具。
背景：本机 PowerShell 的 Add-Type 被安全策略拦截，无法用 P/Invoke 截图，
      因此改用 Python ctypes 直接调 GDI，并用 zlib 手工编码 PNG。

用法：
    python tools/screenshot_window.py <进程名> <输出.png> [等待秒数]

例：
    python tools/screenshot_window.py nofakeip-desktop shot.png 1.5
"""

import ctypes
import ctypes.wintypes as wt
import struct
import subprocess
import sys
import time
import zlib

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32

# ⚠️ 必须在任何窗口 API 之前调用：
# 若进程是「DPI 无感知」的，Windows 会把 GetWindowRect 等返回的坐标按缩放比虚拟化
# （本机 125% 时坐标被除以 1.25），而 BitBlt 用的是物理屏幕坐标 —— 两者对不上，
# 截出来的图会整体偏移且尺寸偏小。声明为 per-monitor DPI aware 即可对齐。
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)  # PROCESS_PER_MONITOR_DPI_AWARE
except Exception:
    try:
        user32.SetProcessDPIAware()
    except Exception:
        pass

SRCCOPY = 0x00CC0020
BI_RGB = 0
DIB_RGB_COLORS = 0


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wt.DWORD),
        ("biWidth", ctypes.c_long),
        ("biHeight", ctypes.c_long),
        ("biPlanes", wt.WORD),
        ("biBitCount", wt.WORD),
        ("biCompression", wt.DWORD),
        ("biSizeImage", wt.DWORD),
        ("biXPelsPerMeter", ctypes.c_long),
        ("biYPelsPerMeter", ctypes.c_long),
        ("biClrUsed", wt.DWORD),
        ("biClrImportant", wt.DWORD),
    ]


def find_window_pid(name: str) -> int:
    """用 tasklist 拿到进程 PID（避免引入 psutil）。"""
    out = subprocess.run(
        ["tasklist", "/FI", f"IMAGENAME eq {name}.exe", "/FO", "CSV", "/NH"],
        capture_output=True,
        text=True,
        errors="ignore",
    ).stdout
    for line in out.splitlines():
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) >= 2 and parts[0].lower().startswith(name.lower()):
            return int(parts[1])
    raise SystemExit(f"找不到进程 {name}.exe（请先启动应用）")


def find_hwnd(pid: int) -> int:
    """枚举顶层窗口，找出属于该 PID 的可见窗口。"""
    result = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        wpid = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
        if wpid.value == pid and user32.IsWindowVisible(hwnd):
            length = user32.GetWindowTextLengthW(hwnd)
            if length > 0:
                result.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    if not result:
        raise SystemExit(f"进程 {pid} 没有可见的顶层窗口")
    return result[0]


def write_png(path: str, width: int, height: int, rows: list[bytes]) -> None:
    """把逐行 RGB 数据写成 PNG（每行以 filter byte 0 开头）。"""
    raw = b"".join(b"\x00" + r for r in rows)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def capture(hwnd: int, path: str, method: str = "screen") -> tuple[int, int]:
    rect = wt.RECT()
    if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
        raise SystemExit("GetWindowRect 失败")
    x, y = rect.left, rect.top
    w, h = rect.right - rect.left, rect.bottom - rect.top

    hdc_screen = user32.GetDC(0)
    hdc_mem = gdi32.CreateCompatibleDC(hdc_screen)
    hbmp = gdi32.CreateCompatibleBitmap(hdc_screen, w, h)
    gdi32.SelectObject(hdc_mem, hbmp)

    if method == "print":
        # PrintWindow + PW_RENDERFULLCONTENT(2)：让窗口把自己渲染进 DC。
        # 对 WebView2 / Chromium 这类硬件合成（甚至走 MPO 硬件叠加层）的窗口，
        # 从屏幕 DC 做 BitBlt 可能只拿到空白；PrintWindow 会触发一次完整重绘，
        # 通常能拿到真实内容。
        if not user32.PrintWindow(hwnd, hdc_mem, 2):
            raise SystemExit("PrintWindow 失败")
    else:
        if not gdi32.BitBlt(hdc_mem, 0, 0, w, h, hdc_screen, x, y, SRCCOPY):
            raise SystemExit("BitBlt 失败")

    # 负高度 = 自顶向下取像素
    bmi = BITMAPINFOHEADER()
    bmi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bmi.biWidth = w
    bmi.biHeight = -h
    bmi.biPlanes = 1
    bmi.biBitCount = 32
    bmi.biCompression = BI_RGB

    bufsize = w * h * 4
    buf = ctypes.create_string_buffer(bufsize)
    if not gdi32.GetDIBits(hdc_mem, hbmp, 0, h, buf, ctypes.byref(bmi), DIB_RGB_COLORS):
        raise SystemExit("GetDIBits 失败")

    # BGRA -> RGB
    rows = []
    stride = w * 4
    for row in range(h):
        line = buf.raw[row * stride : (row + 1) * stride]
        rows.append(bytes(b for i in range(0, stride, 4) for b in (line[i + 2], line[i + 1], line[i])))

    write_png(path, w, h, rows)

    gdi32.DeleteObject(hbmp)
    gdi32.DeleteDC(hdc_mem)
    user32.ReleaseDC(0, hdc_screen)
    return w, h


def main() -> None:
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    name, out = sys.argv[1], sys.argv[2]
    wait = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0
    method = sys.argv[4] if len(sys.argv) > 4 else "print"

    pid = find_window_pid(name)
    hwnd = find_hwnd(pid)

    user32.ShowWindow(hwnd, 9)  # SW_RESTORE

    # 临时置顶：screen 方式抓的是屏幕合成结果，若目标窗口被其它窗口遮挡，
    # 抓到的会是遮挡窗口的内容。HWND_TOPMOST 能保证它在最前，抓完再恢复。
    HWND_TOPMOST, HWND_NOTOPMOST = -1, -2
    SWP_NOMOVE, SWP_NOSIZE = 0x0002, 0x0001
    user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    user32.SetForegroundWindow(hwnd)
    time.sleep(wait)

    try:
        w, h = capture(hwnd, out, method)
    finally:
        user32.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)

    print(f"OK  pid={pid} hwnd={hwnd} {w}x{h} method={method} -> {out}")


if __name__ == "__main__":
    main()
