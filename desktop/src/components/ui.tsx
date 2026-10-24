// 通用 UI 组件。不引第三方 UI 库，保持轻量（见 doc/08-GUI规格.md 第 1 节）。

import React, { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { FakeIpStatus, Region, RiskLevel, ServerBenchResult } from "../types";
import { FAKE_IP_LABEL, PROTOCOL_LABEL, REGION_LABEL, RISK_LABEL } from "../types";

// —————————————————————— 卡片 ——————————————————————

export function Card({
  title,
  children,
  actions,
  className,
}: {
  title?: string;
  children: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={`card ${className ?? ""}`}>
      {(title || actions) && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 12,
            marginBottom: title ? 12 : 0,
          }}
        >
          {title ? <h2 className="card-title" style={{ margin: 0 }}>{title}</h2> : <span />}
          {actions}
        </div>
      )}
      {children}
    </div>
  );
}

// —————————————————————— 徽章 ——————————————————————

export function RegionBadge({ region }: { region: Region }) {
  const cls = region === "china" ? "badge-cn" : region === "global" ? "badge-global" : "badge-neutral";
  return <span className={`badge ${cls}`}>{REGION_LABEL[region]}</span>;
}

export function RiskBadge({ risk }: { risk: RiskLevel }) {
  return <span className={`badge badge-${risk}`}>{RISK_LABEL[risk]}</span>;
}

export function FakeIpBadge({ status }: { status: FakeIpStatus }) {
  const cls =
    status === "normal" ? "badge-low" : status === "detected" ? "badge-cn" : status === "suspicious" ? "badge-medium" : "badge-neutral";
  return <span className={`badge ${cls}`}>{FAKE_IP_LABEL[status]}</span>;
}

/** 延迟徽章：<50ms 绿 / 50–150ms 黄 / >150ms 红（慢=红，不用股票涨跌语义）。 */
export function LatencyBadge({ ms, ok }: { ms: number; ok: boolean }) {
  if (!ok) return <span className="badge badge-neutral">失败</span>;
  const cls = ms < 50 ? "badge-low" : ms <= 150 ? "badge-medium" : "badge-high";
  return <span className={`badge ${cls}`}>{ms.toFixed(0)}ms</span>;
}

// —————————————————————— 表格 ——————————————————————

export interface Column<T> {
  key: string;
  title: string;
  align?: "left" | "right";
  render: (row: T, index: number) => React.ReactNode;
  /** 提供则可点击表头排序 */
  sortValue?: (row: T) => number | string;
}

export function DataTable<T>({
  columns,
  rows,
  empty = "暂无数据",
  initialSortKey,
  initialSortDesc = false,
  rowKey,
}: {
  columns: Column<T>[];
  rows: T[];
  empty?: string;
  initialSortKey?: string;
  initialSortDesc?: boolean;
  rowKey?: (row: T, index: number) => string | number;
}) {
  const [sortKey, setSortKey] = useState<string | undefined>(initialSortKey);
  const [desc, setDesc] = useState(initialSortDesc);

  const sorted = useMemo(() => {
    if (!sortKey) return rows;
    const col = columns.find((c) => c.key === sortKey);
    if (!col?.sortValue) return rows;
    const copy = [...rows];
    copy.sort((a, b) => {
      const va = col.sortValue!(a);
      const vb = col.sortValue!(b);
      const r = typeof va === "number" && typeof vb === "number" ? va - vb : String(va).localeCompare(String(vb));
      return desc ? -r : r;
    });
    return copy;
  }, [rows, columns, sortKey, desc]);

  if (rows.length === 0) return <div className="empty">{empty}</div>;

  return (
    <div className="table-wrap">
      <table className="data">
        <thead>
          <tr>
            {columns.map((c) => (
              <th
                key={c.key}
                className={`${c.align === "right" ? "num" : ""} ${c.sortValue ? "sortable" : ""}`}
                onClick={
                  c.sortValue
                    ? () => {
                        if (sortKey === c.key) setDesc((d) => !d);
                        else {
                          setSortKey(c.key);
                          setDesc(false);
                        }
                      }
                    : undefined
                }
              >
                {c.title}
                {sortKey === c.key ? (desc ? " ↓" : " ↑") : ""}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {sorted.map((row, i) => (
            <tr key={rowKey ? rowKey(row, i) : i}>
              {columns.map((c) => (
                <td key={c.key} className={c.align === "right" ? "num" : ""}>
                  {c.render(row, i)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// —————————————————————— 窄屏结果卡片 ——————————————————————

/**
 * DNS 测速结果的窄屏呈现。
 *
 * 为什么不用表格：结果有 8 列，在 360px 宽的手机上必然要左右滑动才能看全，
 * 用户反馈"显示不全"。窄屏改成每台服务器一张卡片，信息分层纵向排布，
 * 不需要横向滚动；宽屏仍然用表格（信息密度更高）。
 */
export function ServerResultCards({
  rows,
  showDomain = false,
  empty = "暂无结果",
}: {
  rows: (ServerBenchResult & { domain?: string })[];
  showDomain?: boolean;
  empty?: string;
}) {
  if (rows.length === 0) return <div className="empty">{empty}</div>;
  return (
    <div className="result-cards">
      {rows.map((r, i) => (
        <div className="result-card" key={`${r.domain ?? ""}-${r.server_id}-${r.server_name}-${i}`}>
          <div className="result-card-head">
            <span className="result-card-name">{r.server_name}</span>
            <LatencyBadge ms={r.avg_ms} ok={r.ok} />
          </div>

          <div className="result-card-tags">
            <span className="badge badge-neutral">{PROTOCOL_LABEL[r.protocol]}</span>
            <RegionBadge region={r.region} />
            {r.ok ? (
              <span className="badge badge-low">正常</span>
            ) : (
              <span className="badge badge-neutral">{r.error ?? "失败"}</span>
            )}
          </div>

          {showDomain && r.domain && <div className="result-card-line mono hint">{r.domain}</div>}

          {r.ok && (
            <div className="result-card-line hint">
              连接 {r.connect_ms.toFixed(0)}
              {r.tls_ms > 0 ? ` · TLS ${r.tls_ms.toFixed(0)}` : ""}
              {" · 查询 "}
              {r.query_ms.toFixed(0)} ms
            </div>
          )}

          <div className="result-card-line mono hint">{r.answers.length ? r.answers.join(", ") : "—"}</div>
        </div>
      ))}
    </div>
  );
}

// —————————————————————— 其它 ——————————————————————

export function Spinner() {
  return <span className="spinner" />;
}

export function ProgressBar({ done, total, label }: { done: number; total: number; label?: string }) {
  const pct = total === 0 ? 0 : Math.min(100, (done / total) * 100);
  return (
    <div>
      <div className="progress">
        <i style={{ width: `${pct}%` }} />
      </div>
      <div className="hint" style={{ marginTop: 5 }}>
        {done}/{total} {label ?? ""}
      </div>
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </div>
  );
}

export function Stat({
  label,
  value,
  sub,
}: {
  label: string;
  value: React.ReactNode;
  sub?: React.ReactNode;
}) {
  return (
    <div>
      <div className="stat-label">{label}</div>
      <div className="stat-value">{value}</div>
      {sub && <div className="stat-sub">{sub}</div>}
    </div>
  );
}

export function Findings({ findings }: { findings: { code: string; severity: RiskLevel; message: string; evidence: string[] }[] }) {
  if (findings.length === 0) return <div className="empty">未发现异常</div>;
  return (
    <div className="findings">
      {findings.map((f, i) => (
        <div key={i} className={`finding ${f.severity}`}>
          <span className={`badge badge-${f.severity}`}>{RISK_LABEL[f.severity]}</span>
          <div>
            <div className="finding-msg">{f.message}</div>
            <div className="finding-code">{f.code}</div>
            {f.evidence.length > 0 && (
              <div className="hint" style={{ marginTop: 3 }}>
                {f.evidence.join("；")}
              </div>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

// —————————————————————— Toast ——————————————————————

type ToastKind = "info" | "ok" | "warn" | "error";
interface ToastItem {
  id: number;
  kind: ToastKind;
  text: string;
}

const ToastCtx = createContext<(text: string, kind?: ToastKind) => void>(() => {});

/**
 * 模块级 toast 出口，给非 React 代码用（例如 `tasks.ts` 里的长任务）。
 *
 * 为什么需要：长任务在页面被卸载后仍会跑完，此时不能依赖页面里的 `useToast()`
 * 去弹提示；这里由 `ToastProvider` 注册一个全局出口，任务结束时无论用户
 * 当前停在哪个页面都能收到提示。
 */
let toastSink: ((text: string, kind?: ToastKind) => void) | null = null;

export function notify(text: string, kind: ToastKind = "info") {
  toastSink?.(text, kind);
}

export function useToast() {
  return useContext(ToastCtx);
}

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);

  const push = useCallback((text: string, kind: ToastKind = "info") => {
    const id = Date.now() + Math.random();
    setItems((prev) => [...prev, { id, kind, text }]);
    // 错误多留一会儿，方便阅读
    window.setTimeout(() => setItems((prev) => prev.filter((t) => t.id !== id)), kind === "error" ? 9000 : 4500);
  }, []);

  useEffect(() => {
    toastSink = push;
    return () => {
      toastSink = null;
    };
  }, [push]);

  return (
    <ToastCtx.Provider value={push}>
      {children}
      <div className="toasts">
        {items.map((t) => (
          <div key={t.id} className={`toast ${t.kind === "info" ? "" : t.kind}`}>
            {t.text}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
}
