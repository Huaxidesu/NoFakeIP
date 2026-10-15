// 应用外壳：侧边栏导航 + 页面切换 + 全局状态（服务器列表、设置、状态）。
// 窄屏（Android）自动切换为底部 Tab 导航（见 styles.css 的 @media）：
// 底栏只保留 4 个入口，其余页面收进「更多」。

import { useCallback, useEffect, useState } from "react";
import * as api from "./api";
import { Icon, type IconName } from "./components/icons";
import { Spinner, ToastProvider, useToast } from "./components/ui";
import { useRunningTaskLabel } from "./tasks";
import type { AppStatus, DnsServer, Settings } from "./types";
import Dashboard from "./pages/Dashboard";
import DnsBench from "./pages/DnsBench";
import DomainCheck from "./pages/DomainCheck";
import Report from "./pages/Report";
import History from "./pages/History";
import SettingsPage from "./pages/Settings";
import About from "./pages/About";
import More from "./pages/More";
import logo from "./assets/logo.png";

type PageKey = "dashboard" | "bench" | "check" | "report" | "history" | "settings" | "about" | "more";

interface NavEntry {
  key: PageKey;
  /** 图标名，见 components/icons.tsx（内联 SVG，颜色由 CSS 控制） */
  icon: IconName;
  /** 桌面侧栏用的完整名称 */
  label: string;
  /** 窄屏底栏用的短名称（≤4 字，避免换行） */
  mobileLabel: string;
  /** 是否出现在窄屏底栏 */
  mobile?: boolean;
}

const NAV: NavEntry[] = [
  { key: "dashboard", icon: "dashboard", label: "首页", mobileLabel: "首页", mobile: true },
  { key: "bench", icon: "bolt", label: "DNS 测速", mobileLabel: "测速", mobile: true },
  { key: "check", icon: "target", label: "域名检测", mobileLabel: "检测", mobile: true },
  { key: "report", icon: "doc", label: "推荐与报告", mobileLabel: "报告" },
  { key: "history", icon: "clock", label: "历史记录", mobileLabel: "历史" },
  { key: "settings", icon: "sliders", label: "设置", mobileLabel: "设置" },
  { key: "about", icon: "info", label: "关于", mobileLabel: "关于" },
];

/** 窄屏底栏的第 4 项：其余页面的入口 */
const MORE: NavEntry = { key: "more", icon: "more", label: "更多", mobileLabel: "更多" };

/** 当前页在窄屏底栏上应该高亮哪一项 */
function mobileActiveKey(page: PageKey): PageKey {
  return NAV.some((n) => n.key === page && n.mobile) ? page : "more";
}

export interface AppCtx {
  status: AppStatus | null;
  servers: DnsServer[];
  settings: Settings | null;
  refreshStatus: () => Promise<void>;
  refreshServers: () => Promise<void>;
  refreshSettings: () => Promise<void>;
}

function Shell() {
  const toast = useToast();
  const [page, setPage] = useState<PageKey>("dashboard");
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [servers, setServers] = useState<DnsServer[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [booting, setBooting] = useState(true);

  const refreshStatus = useCallback(async () => {
    setStatus(await api.getStatus());
  }, []);

  const refreshServers = useCallback(async () => {
    setServers(await api.listServers(false));
  }, []);

  const refreshSettings = useCallback(async () => {
    setSettings(await api.getSettings());
  }, []);

  useEffect(() => {
    (async () => {
      try {
        await Promise.all([refreshStatus(), refreshServers(), refreshSettings()]);
      } catch (e) {
        toast(`初始化失败：${(e as Error).message}`, "error");
      } finally {
        setBooting(false);
      }
    })();
    // 仅在首次挂载时执行
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const ctx: AppCtx = { status, servers, settings, refreshStatus, refreshServers, refreshSettings };
  const activeMobile = mobileActiveKey(page);
  const taskLabel = useRunningTaskLabel();

  // 同一个页面会同时渲染桌面项与窄屏项，key 必须带前缀区分，否则同层重复
  const navButton = (n: NavEntry, active: boolean, extraClass: string, keyPrefix: string) => (
    <button
      key={`${keyPrefix}-${n.key}`}
      className={`nav-item ${active ? "active" : ""} ${extraClass}`}
      aria-label={n.label}
      onClick={() => setPage(n.key)}
    >
      <span className="nav-icon-wrap">
        <Icon name={n.icon} className="nav-icon" />
      </span>
      <span className="nav-label-desktop">{n.label}</span>
      <span className="nav-label-mobile">{n.mobileLabel}</span>
    </button>
  );

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="brand">
          <img className="brand-logo" src={logo} alt="" />
          <div>
            <div className="brand-name">NoFakeIP</div>
            <div className="brand-sub">DNS 诊断与测速</div>
          </div>
        </div>

        {/* 桌面侧栏：全部页面直达 */}
        {NAV.map((n) => navButton(n, page === n.key, "nav-desktop-only", "d"))}

        {/* 窄屏底栏：前 3 项 + 「更多」 */}
        {NAV.filter((n) => n.mobile).map((n) => navButton(n, page === n.key, "nav-mobile-only", "m"))}
        {navButton(MORE, activeMobile === "more", "nav-mobile-only", "m")}

        <div className="sidebar-foot">
          {status ? `v${status.version} · 节点 ${status.enabled_count}/${status.server_count}` : "加载中…"}
        </div>
      </nav>

      <main className="main">
        {/* 长任务在后台继续跑：无论当前在哪个页面都显示进度，避免误以为任务被中止 */}
        {taskLabel && (
          <div className="task-banner" role="status">
            <Spinner />
            <span>{taskLabel}</span>
            <span className="hint">可自由切换页面，任务会在后台继续</span>
          </div>
        )}

        {booting ? (
          <div className="empty">正在加载…</div>
        ) : page === "dashboard" ? (
          <Dashboard ctx={ctx} onGoto={(p) => setPage(p as PageKey)} />
        ) : page === "bench" ? (
          <DnsBench ctx={ctx} />
        ) : page === "check" ? (
          <DomainCheck ctx={ctx} />
        ) : page === "report" ? (
          <Report ctx={ctx} />
        ) : page === "history" ? (
          <History ctx={ctx} />
        ) : page === "settings" ? (
          <SettingsPage ctx={ctx} />
        ) : page === "about" ? (
          <About ctx={ctx} />
        ) : (
          <More onGoto={(p) => setPage(p as PageKey)} />
        )}
      </main>
    </div>
  );
}

export default function App() {
  return (
    <ToastProvider>
      <Shell />
    </ToastProvider>
  );
}
