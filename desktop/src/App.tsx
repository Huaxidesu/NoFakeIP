// 应用外壳：侧边栏导航 + 页面切换 + 全局状态（服务器列表、设置、状态）。
// 窄屏（Android）自动切换为底部 Tab 导航（见 styles.css 的 @media）。

import { useCallback, useEffect, useState } from "react";
import * as api from "./api";
import { ToastProvider, useToast } from "./components/ui";
import type { AppStatus, DnsServer, Settings } from "./types";
import Dashboard from "./pages/Dashboard";
import DnsBench from "./pages/DnsBench";
import DomainCheck from "./pages/DomainCheck";
import Report from "./pages/Report";
import History from "./pages/History";
import SettingsPage from "./pages/Settings";

type PageKey = "dashboard" | "bench" | "check" | "report" | "history" | "settings";

const NAV: { key: PageKey; icon: string; label: string }[] = [
  { key: "dashboard", icon: "◧", label: "首页" },
  { key: "bench", icon: "⚡", label: "DNS 测速" },
  { key: "check", icon: "◎", label: "域名检测" },
  { key: "report", icon: "▤", label: "推荐与报告" },
  { key: "history", icon: "◷", label: "历史记录" },
  { key: "settings", icon: "⚙", label: "设置" },
];

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

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="brand">
          <div className="brand-mark">N</div>
          <div>
            <div className="brand-name">NoFakeIP</div>
            <div className="brand-sub">DNS 诊断与测速</div>
          </div>
        </div>
        {NAV.map((n) => (
          <button
            key={n.key}
            className={`nav-item ${page === n.key ? "active" : ""}`}
            onClick={() => setPage(n.key)}
          >
            <span className="nav-icon">{n.icon}</span>
            <span>{n.label}</span>
          </button>
        ))}
        <div className="sidebar-foot">
          {status ? `v${status.version} · 节点 ${status.enabled_count}/${status.server_count}` : "加载中…"}
        </div>
      </nav>

      <main className="main">
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
        ) : (
          <SettingsPage ctx={ctx} />
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
