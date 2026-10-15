#!/usr/bin/env python3
"""窄屏（手机竖屏）界面与长任务行为回归校验。

为什么需要：桌面端与 Android 共用同一套前端，窄屏走底部 Tab 导航。
底栏项数、按钮尺寸、结果表格是否溢出、切换页面会不会丢任务这类问题
**只在窄屏或运行期出现**，桌面截图看不出来。

本脚本用 CDP 把真实浏览器切成 360×800 手机视口，注入一个假 Tauri 后端
（见 `tools/_fake_tauri.js`），然后逐项断言。

前置：
    1) 本地静态服务：python -m http.server 4322 --directory desktop/dist
    2) 带调试端口的浏览器：
       msedge --headless=new --remote-debugging-port=9223 \
              --user-data-dir=<临时目录> http://127.0.0.1:4322/

用法：
    python tools/verify-mobile-ui.py [调试端口] [页面地址前缀]
"""

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from cdp_probe import WS, http_json  # noqa: E402  （同目录复用极简 CDP 客户端）

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STUB = os.path.join(ROOT, "tools", "_fake_tauri.js")

VIEWPORT_W = 360
VIEWPORT_H = 800

failures = []


def check(name, ok, detail=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(name)


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9223
    prefix = sys.argv[2] if len(sys.argv) > 2 else "http://127.0.0.1:4322/"

    target = None
    for _ in range(20):
        try:
            for t in http_json(port, "/json/list"):
                if t.get("type") == "page" and t.get("url", "").startswith(prefix):
                    target = t
                    break
        except Exception:
            pass
        if target:
            break
        time.sleep(0.5)
    if not target:
        raise SystemExit("找不到页面目标：确认静态服务与浏览器调试端口已启动")

    ws = WS(target["webSocketDebuggerUrl"])
    # 截图 / 重载偶发超过 30s（尤其从大视口切回手机视口时），放宽 socket 超时
    ws.sock.settimeout(120)
    counter = [0]

    def call(method, params=None):
        counter[0] += 1
        return ws.call(counter[0], method, params or {})

    def evaluate(expr, await_promise=False):
        params = {"expression": expr, "returnByValue": True}
        if await_promise:
            params["awaitPromise"] = True
        res = call("Runtime.evaluate", params)
        return res.get("result", {}).get("result", {}).get("value")

    def js(expr, await_promise=False):
        raw = evaluate(expr, await_promise)
        if raw is None:
            return None
        try:
            return json.loads(raw)
        except (TypeError, ValueError):
            return raw

    def wait_for(expr, timeout=15, label=""):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if evaluate(expr):
                return True
            time.sleep(0.25)
        print(f"       （等待超时：{label or expr}）")
        return False

    def click_mobile_nav(label):
        return evaluate(
            """(async () => {
              const item = Array.from(document.querySelectorAll('.nav-item'))
                .filter(e => e.offsetParent !== null)
                .find(e => e.querySelector('.nav-label-mobile').textContent.trim() === %s);
              if (!item) return false;
              item.click();
              await new Promise(r => setTimeout(r, 250));
              return true;
            })()""" % json.dumps(label),
            await_promise=True,
        )

    def click_text(selector, text):
        return evaluate(
            """(async () => {
              const el = Array.from(document.querySelectorAll(%s))
                .find(e => e.textContent.trim().includes(%s));
              if (!el) return false;
              el.click();
              return true;
            })()""" % (json.dumps(selector), json.dumps(text)),
            await_promise=True,
        )

    # 注入假后端：必须在页面脚本之前
    with open(STUB, "r", encoding="utf-8") as f:
        stub_src = f.read()
    call("Page.enable")
    call("Page.addScriptToEvaluateOnNewDocument", {"source": stub_src})
    call("Emulation.setDeviceMetricsOverride", {
        "width": VIEWPORT_W, "height": VIEWPORT_H, "deviceScaleFactor": 3, "mobile": True,
    })
    call("Page.reload", {"ignoreCache": True})
    wait_for("!!document.querySelector('.quick-tile')", timeout=20, label="首页渲染")

    print("=== 1. 窄屏底栏 ===")
    nav = js("""JSON.stringify({
      vw: innerWidth,
      visible: Array.from(document.querySelectorAll('.nav-item'))
        .filter(e => e.offsetParent !== null)
        .map(e => ({ label: e.querySelector('.nav-label-mobile').textContent.trim(),
                     w: Math.round(e.getBoundingClientRect().width),
                     right: Math.round(e.getBoundingClientRect().right) }))
    })""")
    labels = [x["label"] for x in nav["visible"]]
    check("窄屏底栏只有 4 项", len(labels) == 4, f"实际 {len(labels)} 项：{labels}")
    check("底栏标签为 首页/测速/检测/更多", labels == ["首页", "测速", "检测", "更多"], str(labels))
    check("底栏没有横向溢出", all(x["right"] <= nav["vw"] + 1 for x in nav["visible"]), f"视口 {nav['vw']}px")
    check("底栏每项触控宽度 ≥ 72px", all(x["w"] >= 72 for x in nav["visible"]), str([x["w"] for x in nav["visible"]]))

    print("\n=== 2. 导航图标风格 ===")
    # 图标已于 2026-10-15 改为**内联 SVG**（components/icons.tsx）：
    # 此前用 ⚡(U+26A1) / ⚙(U+2699) 字符，属 emoji 码位，Android 上会渲染成
    # 彩色表情（补 U+FE0E 也压不住），且依赖系统字体覆盖。
    # 因此断言改为：每个可见导航项的图标必须是 <svg>，且不含任何文字字形内容。
    icons = js("""JSON.stringify(Array.from(document.querySelectorAll('.nav-item'))
      .filter(e => e.offsetParent !== null)
      .map(e => { const svg = e.querySelector('.nav-icon'); const b = svg ? svg.getBoundingClientRect() : null;
                  return { label: e.querySelector('.nav-label-mobile').textContent.trim(),
                   isSvg: svg?.tagName.toLowerCase() === 'svg',
                   text: (svg?.textContent ?? '').trim(),
                   cps: Array.from(svg?.textContent ?? '').map(c => c.codePointAt(0)),
                   w: b ? Math.round(b.width) : 0, h: b ? Math.round(b.height) : 0 }; }))""")
    check("导航项数量与底栏一致", len(icons) == 4, f"实际 {len(icons)}：{[i['label'] for i in icons]}")
    check("导航图标全部为内联 SVG", all(i["isSvg"] for i in icons),
          str([(i["label"], i["isSvg"]) for i in icons]))
    check("导航图标不含文字字形（无 emoji / 字符图标）",
          all(i["text"] == "" for i in icons),
          str([(i["label"], i["text"]) for i in icons]))
    check("导航图标不含彩色 emoji 码位",
          all(all(c < 0x1F000 for c in i["cps"]) for i in icons),
          str([(i["label"], [hex(c) for c in i["cps"]]) for i in icons]))
    # 关键回归：内联 <svg> 只有 viewBox 时 width:auto 会塌成 0×0（真机上就是「图标不见了」）。
    # 必须断言实际渲染尺寸 > 0，光看 tagName 是 <svg> 抓不到这个 bug。
    check("导航图标实际渲染尺寸非 0（宽高均 ≥ 12px）",
          all(i["w"] >= 12 and i["h"] >= 12 for i in icons),
          str([(i["label"], i["w"], i["h"]) for i in icons]))

    print("\n=== 3. 首页主要操作 ===")
    home = js("""JSON.stringify({
      mainScroll: document.querySelector('.main').scrollHeight,
      mainClient: document.querySelector('.main').clientHeight,
      tiles: Array.from(document.querySelectorAll('.quick-tile'))
        .filter(e => e.offsetParent !== null)
        .map(e => ({ title: e.querySelector('.quick-title').textContent.trim(),
                     h: Math.round(e.getBoundingClientRect().height),
                     primary: e.classList.contains('quick-tile-primary') })),
      oldSmallButtons: !!document.querySelector('.home-actions-desktop, .home-actions-mobile')
    })""")
    titles = [t["title"] for t in home["tiles"]]
    check("首页有两个大幅操作卡片", len(titles) == 2, str(titles))
    check("其中包含「域名检测」", "域名检测" in titles, str(titles))
    check("域名检测是主操作（实心高亮）",
          any(t["primary"] and t["title"] == "域名检测" for t in home["tiles"]), str(home["tiles"]))
    check("操作卡片高度 ≥ 70px", len(home["tiles"]) == 2 and all(t["h"] >= 70 for t in home["tiles"]),
          str([t["h"] for t in home["tiles"]]))
    # 同类 bug：卡片图标也是内联 SVG，必须断言渲染尺寸非 0
    quick_icons = js("""JSON.stringify(Array.from(document.querySelectorAll('.quick-icon'))
      .filter(e => e.offsetParent !== null)
      .map(e => { const s = e.querySelector('svg'); const b = s ? s.getBoundingClientRect() : null;
                  return { isSvg: !!s, w: b ? Math.round(b.width) : 0, h: b ? Math.round(b.height) : 0 }; }))""")
    check("首页操作卡片图标为 SVG 且渲染尺寸非 0",
          len(quick_icons) == 2 and all(i["isSvg"] and i["w"] >= 12 and i["h"] >= 12 for i in quick_icons),
          str([(i["isSvg"], i["w"], i["h"]) for i in quick_icons]))
    check("旧的右上角小按钮已移除", home["oldSmallButtons"] is False)
    over = home["mainScroll"] - home["mainClient"]
    check("首页内容不超出一屏（允许 ≤8px 取整误差）", over <= 8,
          f"内容 {home['mainScroll']}px / 可视 {home['mainClient']}px（超出 {over}px）")

    print("\n=== 4. 窄屏测速结果展示 ===")
    check("能切到「测速」页", click_mobile_nav("测速") is True)
    wait_for("!!document.querySelector('.quick-start, .page-title')", timeout=8, label="测速页")
    check("能点到「开始测试」", click_text("button", "开始测试") is True)
    # 任务进行中：切走再切回来，验证结果不丢
    time.sleep(0.4)
    banner = evaluate("!!document.querySelector('.task-banner')")
    check("任务进行中显示全局进度条", banner is True)
    check("任务进行中能切到首页", click_mobile_nav("首页") is True)
    time.sleep(2.2)
    check("切回「测速」页", click_mobile_nav("测速") is True)
    time.sleep(0.5)

    bench = js("""JSON.stringify({
      running: !!document.querySelector('.task-banner'),
      desktopVisible: Array.from(document.querySelectorAll('.desktop-only')).some(e => e.offsetParent !== null),
      mobileVisible: Array.from(document.querySelectorAll('.mobile-only')).some(e => e.offsetParent !== null),
      cards: Array.from(document.querySelectorAll('.mobile-only .result-card'))
        .filter(e => e.offsetParent !== null)
        .map(e => e.querySelector('.result-card-name').textContent.trim()),
      docScroll: document.documentElement.scrollWidth,
      vw: innerWidth
    })""")
    check("切页期间任务未中断，回来后已有结果", len(bench["cards"]) == 4, str(bench["cards"]))
    check("任务结束后全局进度条消失", bench["running"] is False)
    check("窄屏隐藏宽屏表格", bench["desktopVisible"] is False)
    check("窄屏渲染结果卡片", bench["mobileVisible"] is True)
    check("测速结果页无横向溢出（不需要左右滑）",
          bench["docScroll"] <= bench["vw"] + 1,
          f"内容宽 {bench['docScroll']}px / 视口 {bench['vw']}px")

    print("\n=== 5. 「更多」与「关于」 ===")
    check("能点到「更多」", click_mobile_nav("更多") is True)
    wait_for("!!document.querySelector('.menu-item')", timeout=8, label="更多页")
    more_page = js("""JSON.stringify({
      title: document.querySelector('.page-title')?.textContent ?? null,
      items: Array.from(document.querySelectorAll('.menu-item .menu-title')).map(e => e.textContent.trim()),
      activeNav: document.querySelector('.nav-item.active .nav-label-mobile')?.textContent.trim() ?? null
    })""")
    check("「更多」页可打开", more_page["title"] == "更多", str(more_page["title"]))
    check("「更多」列出 4 个入口",
          more_page["items"] == ["推荐与报告", "历史记录", "设置", "关于"], str(more_page["items"]))
    check("非底栏页面时底栏高亮「更多」", more_page["activeNav"] == "更多", str(more_page["activeNav"]))
    menu_icons = js("""JSON.stringify(Array.from(document.querySelectorAll('.menu-icon'))
      .filter(e => e.offsetParent !== null)
      .map(e => { const s = e.querySelector('svg'); const b = s ? s.getBoundingClientRect() : null;
                  return { isSvg: !!s, w: b ? Math.round(b.width) : 0, h: b ? Math.round(b.height) : 0 }; }))""")
    check("「更多」页图标为 SVG 且渲染尺寸非 0",
          len(menu_icons) == 4 and all(i["isSvg"] and i["w"] >= 12 and i["h"] >= 12 for i in menu_icons),
          str([(i["isSvg"], i["w"], i["h"]) for i in menu_icons]))

    js("""(async () => {
      const t = Array.from(document.querySelectorAll('.menu-item'))
        .find(e => e.querySelector('.menu-title').textContent.trim() === '关于');
      t.click();
      await new Promise(r => setTimeout(r, 250));
      return 'ok';
    })()""", await_promise=True)
    wait_for("!!document.querySelector('.about-logo')", timeout=8, label="关于页")
    about = js("""JSON.stringify({
      title: document.querySelector('.page-title')?.textContent ?? null,
      hasLogo: !!document.querySelector('.about-logo'),
      text: document.body.innerText
    })""")
    check("关于页可打开", about["title"] == "关于", str(about["title"]))
    check("关于页显示应用图标", about["hasLogo"] is True)
    check("关于页已删除 Clash 说明文字", "Clash" not in about["text"])

    # 全量扫描：推荐与报告页等其它页面也不应出现 Clash 字样
    js("""(async () => {
      const t = Array.from(document.querySelectorAll('.menu-item, .nav-item'));
      const report = t.find(e => ((e.querySelector('.menu-title, .nav-label, .nav-label-mobile')?.textContent) || '').includes('报告'));
      if (report) report.click();
      await new Promise(r => setTimeout(r, 300));
      return 'ok';
    })()""", await_promise=True)
    time.sleep(0.6)
    report_text = js("""document.body.innerText""")
    check("报告页已删除 Clash 说明文字", "Clash" not in report_text)

    print("\n=== 6. 域名检测 ===")
    check("能点到「检测」", click_mobile_nav("检测") is True)
    wait_for("!!document.querySelector('input[placeholder]')", timeout=8, label="检测页")
    check("能点到「开始检测」", click_text("button", "开始检测") is True)
    time.sleep(2.2)
    detect = js("""JSON.stringify({
      hasDiag: !!document.querySelector('.badge'),
      cards: document.querySelectorAll('.mobile-only .result-card').length,
      docScroll: document.documentElement.scrollWidth,
      vw: innerWidth
    })""")
    check("检测结果已渲染", detect["hasDiag"] is True)
    check("检测结果用卡片呈现", detect["cards"] == 4, f"卡片数 {detect['cards']}")
    check("检测页无横向溢出", detect["docScroll"] <= detect["vw"] + 1,
          f"内容宽 {detect['docScroll']}px / 视口 {detect['vw']}px")

    ws.close()

    print()
    if failures:
        print(f"❌ {len(failures)} 项未通过：" + "；".join(failures))
        raise SystemExit(1)
    print("✅ 窄屏界面与长任务行为校验全部通过")


if __name__ == "__main__":
    main()
