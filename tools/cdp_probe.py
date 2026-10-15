#!/usr/bin/env python3
"""通过 CDP 探测 WebView 里真正渲染出的内容（仅标准库）。

用途：
  1) 桌面端：本机 GDI 截图抓不到 WebView2 的合成层，只能靠 CDP 看 DOM。
     （注意：Tauri 会覆盖 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS，
      需要在 tauri.conf.json 的 additionalBrowserArgs 里开调试端口；
      本机实测端口没能打开，见 doc/11 第 6.6 节。）
  2) Android：Tauri 的 debug 构建会调用 setWebContentsDebuggingEnabled(true)，
     此时可以通过 adb 把 WebView 的调试端口转发到本机：

       adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>
       python tools/cdp_probe.py 9222 shot.png

     <pid> 是应用进程号，可用 `adb shell pidof com.nofakeip.desktop` 取。

用法：
    python tools/cdp_probe.py [端口] [输出截图.png] [JS 表达式]
"""

import base64
import json
import os
import socket
import struct
import sys
import time
import urllib.request

EVAL_EXPR_DEFAULT = """(() => {
  const q = (s) => document.querySelectorAll(s).length;
  return JSON.stringify({
    url: location.href,
    title: document.title,
    hasInternals: typeof window.__TAURI_INTERNALS__,
    internalsKeys: window.__TAURI_INTERNALS__ ? Object.keys(window.__TAURI_INTERNALS__).join(',') : null,
    isTauri: window.isTauri === true,
    rootChildren: document.getElementById('root')?.children.length ?? -1,
    navItems: q('.nav-item'),
    cards: q('.card'),
    pageTitle: document.querySelector('.page-title')?.textContent ?? null,
    bodyBg: getComputedStyle(document.body).backgroundColor,
    innerH: window.innerHeight,
    innerW: window.innerWidth,
    text: (document.body.innerText || '').replace(/\\s+/g, ' ').slice(0, 300),
  });
})()"""


def http_json(port: int, path: str):
    # 本机环境有 HTTP 代理，访问 localhost 必须绕开，否则会被代理拦掉
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(f"http://127.0.0.1:{port}{path}", timeout=5) as r:
        return json.loads(r.read().decode("utf-8"))


def pick_page_target(port: int) -> dict:
    for _ in range(20):
        try:
            targets = http_json(port, "/json")
        except Exception:
            time.sleep(0.5)
            continue
        pages = [t for t in targets if t.get("type") == "page" and t.get("webSocketDebuggerUrl")]
        if pages:
            return pages[0]
        time.sleep(0.5)
    raise SystemExit("没有找到可调试的 page 目标（端口未转发或应用未启动）")


class WS:
    """极简 WebSocket 客户端（文本帧收发，自动应答 ping）。"""

    def __init__(self, url: str):
        assert url.startswith("ws://"), url
        rest = url[len("ws://") :]
        hostport, _, path = rest.partition("/")
        path = "/" + path
        host, _, port = hostport.partition(":")
        self.sock = socket.create_connection((host, int(port or 80)), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {hostport}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode())
        self.buf = b""
        while b"\r\n\r\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise SystemExit("WebSocket 握手失败：连接被关闭")
            self.buf += chunk
        head, _, self.buf = self.buf.partition(b"\r\n\r\n")
        if b"101" not in head.split(b"\r\n")[0]:
            raise SystemExit(f"WebSocket 握手失败：{head[:200]!r}")

    def _read(self, n: int) -> bytes:
        while len(self.buf) < n:
            chunk = self.sock.recv(1 << 16)
            if not chunk:
                raise SystemExit("WebSocket 连接中断")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def send_text(self, text: str) -> None:
        payload = text.encode()
        header = bytearray([0x81])  # FIN + text
        n = len(payload)
        if n < 126:
            header.append(0x80 | n)
        elif n < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", n)
        mask = os.urandom(4)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(bytes(header) + masked)

    def recv_message(self):
        while True:
            b0, b1 = self._read(2)
            opcode = b0 & 0x0F
            masked = b1 & 0x80
            length = b1 & 0x7F
            if length == 126:
                length = struct.unpack(">H", self._read(2))[0]
            elif length == 127:
                length = struct.unpack(">Q", self._read(8))[0]
            mask = self._read(4) if masked else b""
            payload = self._read(length)
            if masked:
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            if opcode == 0x9:  # ping → pong
                self.sock.sendall(b"\x8a\x80" + os.urandom(4))
                continue
            if opcode == 0x8:
                raise SystemExit("对端关闭了 WebSocket")
            return opcode, payload

    def call(self, msg_id: int, method: str, params: dict | None = None) -> dict:
        self.send_text(json.dumps({"id": msg_id, "method": method, "params": params or {}}))
        while True:
            opcode, payload = self.recv_message()
            if opcode != 0x1:
                continue
            data = json.loads(payload.decode("utf-8"))
            if data.get("id") == msg_id:
                return data

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9222
    shot = sys.argv[2] if len(sys.argv) > 2 else None
    expr = sys.argv[3] if len(sys.argv) > 3 else EVAL_EXPR_DEFAULT

    target = pick_page_target(port)
    print(f"target: {target.get('title')!r}  {target.get('url')}")

    ws = WS(target["webSocketDebuggerUrl"])
    try:
        res = ws.call(1, "Runtime.evaluate", {"expression": expr, "returnByValue": True})
        raw = res.get("result", {}).get("result", {}).get("value")
        if raw is None:
            print("Runtime.evaluate 失败：", json.dumps(res, ensure_ascii=False)[:800])
        else:
            try:
                info = json.loads(raw)
                print("\n--- 渲染结果 ---")
                for k, v in info.items():
                    print(f"  {k:16} = {v}")
            except (TypeError, ValueError):
                print("\n--- 求值结果 ---")
                print(raw)

        if shot:
            shot_res = ws.call(2, "Page.captureScreenshot", {"format": "png"})
            data = shot_res.get("result", {}).get("data")
            if data:
                with open(shot, "wb") as f:
                    f.write(base64.b64decode(data))
                print(f"\n截图已保存：{shot}")
            else:
                print("\n截图失败：", json.dumps(shot_res, ensure_ascii=False)[:300])
    finally:
        ws.close()


if __name__ == "__main__":
    main()
