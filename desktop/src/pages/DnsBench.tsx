// DNS 测速页：选域名 + 选节点 → 测速 → 表格 + 延迟对比图 + 导出 CSV。

import { useMemo, useState } from "react";
import type { AppCtx } from "../App";
import * as api from "../api";
import { EChart, themeColors } from "../components/EChart";
import { Card, DataTable, Field, LatencyBadge, RegionBadge, Spinner, useToast, type Column } from "../components/ui";
import type { DomainBenchResult, ServerBenchResult } from "../types";
import { PROTOCOL_LABEL } from "../types";

interface Row extends ServerBenchResult {
  domain: string;
}

export default function DnsBench({ ctx }: { ctx: AppCtx }) {
  const toast = useToast();
  const [domains, setDomains] = useState("www.baidu.com, github.com");
  const [selected, setSelected] = useState<number[] | null>(null);
  const [rounds, setRounds] = useState<number | null>(null);
  const [timeoutSecs, setTimeoutSecs] = useState<number | null>(null);
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<DomainBenchResult[]>([]);

  const enabledServers = useMemo(() => ctx.servers.filter((s) => s.enabled), [ctx.servers]);
  const chosen = selected ?? enabledServers.map((s) => s.id);

  const toggle = (id: number) => {
    const next = chosen.includes(id) ? chosen.filter((x) => x !== id) : [...chosen, id];
    setSelected(next);
  };

  const domainList = domains
    .split(/[\s,，;；]+/)
    .map((d) => d.trim())
    .filter(Boolean);

  const run = async () => {
    if (domainList.length === 0) {
      toast("请先输入至少一个域名", "warn");
      return;
    }
    if (chosen.length === 0) {
      toast("请至少选择一个 DNS 节点", "warn");
      return;
    }
    setRunning(true);
    try {
      const r = await api.runBenchmark(domainList, chosen, {
        rounds: rounds ?? ctx.settings?.rounds ?? 3,
        timeout_secs: timeoutSecs ?? ctx.settings?.timeout_secs ?? 5,
        concurrency: ctx.settings?.concurrency ?? 16,
      });
      setResults(r);
      const ok = r.reduce((n, d) => n + d.results.filter((s) => s.ok).length, 0);
      toast(`测速完成：${r.length} 个域名，${ok} 条成功结果`, "ok");
      await ctx.refreshStatus();
    } catch (e) {
      toast(`测速失败：${(e as Error).message}`, "error");
    } finally {
      setRunning(false);
    }
  };

  const rows: Row[] = useMemo(
    () => results.flatMap((d) => d.results.map((s) => ({ ...s, domain: d.domain }))),
    [results],
  );

  const columns: Column<Row>[] = [
    { key: "domain", title: "域名", render: (r) => <span className="mono">{r.domain}</span> },
    { key: "server", title: "服务器", render: (r) => r.server_name },
    { key: "protocol", title: "协议", render: (r) => PROTOCOL_LABEL[r.protocol] },
    { key: "region", title: "区域", render: (r) => <RegionBadge region={r.region} /> },
    {
      key: "latency",
      title: "平均延迟",
      align: "right",
      render: (r) => <LatencyBadge ms={r.avg_ms} ok={r.ok} />,
      sortValue: (r) => (r.ok ? r.avg_ms : Number.MAX_SAFE_INTEGER),
    },
    {
      key: "seg",
      title: "连接 / TLS / 查询",
      align: "right",
      render: (r) =>
        r.ok ? (
          <span className="hint">
            {r.connect_ms.toFixed(0)} / {r.tls_ms > 0 ? `${r.tls_ms.toFixed(0)} / ` : "— / "}
            {r.query_ms.toFixed(0)} ms
          </span>
        ) : (
          <span className="hint">—</span>
        ),
    },
    {
      key: "status",
      title: "状态",
      render: (r) => (r.ok ? <span className="badge badge-low">正常</span> : <span className="badge badge-neutral">{r.error ?? "失败"}</span>),
    },
    {
      key: "answers",
      title: "返回 IP",
      render: (r) => <span className="mono hint">{r.answers.join(", ") || "—"}</span>,
    },
  ];

  // 延迟对比图：按服务器聚合（跨域名取平均）
  const chartOption = useMemo(() => {
    const c = themeColors();
    const agg = new Map<string, { total: number; n: number }>();
    for (const r of rows) {
      if (!r.ok) continue;
      const cur = agg.get(r.server_name) ?? { total: 0, n: 0 };
      cur.total += r.avg_ms;
      cur.n += 1;
      agg.set(r.server_name, cur);
    }
    const items = [...agg.entries()]
      .map(([name, v]) => ({ name, ms: v.total / v.n }))
      .sort((a, b) => a.ms - b.ms);
    return {
      grid: { left: 8, right: 40, top: 10, bottom: 8, containLabel: true },
      tooltip: { trigger: "axis", valueFormatter: (v: number) => `${v.toFixed(0)} ms` },
      xAxis: {
        type: "value",
        axisLabel: { color: c.fgMuted, formatter: "{value} ms" },
        splitLine: { lineStyle: { color: c.border } },
      },
      yAxis: {
        type: "category",
        data: items.map((i) => i.name),
        axisLabel: { color: c.fgMuted },
        axisLine: { lineStyle: { color: c.border } },
      },
      series: [
        {
          type: "bar",
          data: items.map((i) => ({
            value: Number(i.ms.toFixed(1)),
            itemStyle: { color: i.ms < 50 ? c.ok : i.ms <= 150 ? c.warn : c.danger, borderRadius: [0, 4, 4, 0] },
          })),
          barMaxWidth: 18,
          label: { show: true, position: "right", color: c.fgMuted, formatter: "{c} ms", fontSize: 11 },
        },
      ],
    };
  }, [rows]);

  const exportCsv = async () => {
    if (results.length === 0) return;
    try {
      const content = await api.generateReport("last", null, "csv");
      const path = await api.pickSavePath("nofakeip-bench.csv", ["csv"]);
      if (!path) return;
      await api.saveTextFile(content, path);
      toast(`已导出：${path}`, "ok");
    } catch (e) {
      toast(`导出失败：${(e as Error).message}`, "error");
    }
  };

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">DNS 测速</h1>
          <p className="page-desc">
            对指定域名同时向多个 DNS 发起真实查询，测量连接 / TLS / 查询分段耗时。
          </p>
        </div>
        <div className="btn-row">
          <button className="btn" onClick={exportCsv} disabled={results.length === 0}>
            导出 CSV
          </button>
          <button className="btn btn-primary" onClick={run} disabled={running}>
            {running ? (
              <>
                <Spinner /> 测速中…
              </>
            ) : (
              "开始测试"
            )}
          </button>
        </div>
      </div>

      <Card title="测试参数">
        <Field label="域名（逗号或换行分隔）">
          <textarea rows={2} value={domains} onChange={(e) => setDomains(e.target.value)} />
        </Field>
        <div className="row" style={{ gap: 16 }}>
          <Field label="轮次">
            <input
              type="number"
              min={1}
              max={10}
              style={{ width: 90 }}
              value={rounds ?? ctx.settings?.rounds ?? 3}
              onChange={(e) => setRounds(Number(e.target.value))}
            />
          </Field>
          <Field label="超时（秒）">
            <input
              type="number"
              min={1}
              max={30}
              style={{ width: 90 }}
              value={timeoutSecs ?? ctx.settings?.timeout_secs ?? 5}
              onChange={(e) => setTimeoutSecs(Number(e.target.value))}
            />
          </Field>
          <div className="hint">
            已选 {chosen.length} / {enabledServers.length} 个已启用节点
          </div>
        </div>

        <div className="field">
          <label>参与测试的 DNS 节点</label>
          <div className="row">
            {enabledServers.length === 0 && (
              <span className="hint">没有已启用的节点，请到「设置」里启用或添加。</span>
            )}
            {enabledServers.map((s) => (
              <label key={s.id} className="badge badge-neutral" style={{ cursor: "pointer", gap: 6 }}>
                <input
                  type="checkbox"
                  checked={chosen.includes(s.id)}
                  onChange={() => toggle(s.id)}
                  style={{ margin: 0 }}
                />
                {s.name}
              </label>
            ))}
          </div>
        </div>
      </Card>

      {rows.length > 0 && (
        <Card title="延迟对比">
          <EChart option={chartOption} height={Math.max(160, 34 * new Set(rows.map((r) => r.server_name)).size + 60)} />
        </Card>
      )}

      <Card title="测速结果">
        <DataTable
          columns={columns}
          rows={rows}
          empty="还没有结果，填好参数后点击「开始测试」"
          initialSortKey="latency"
          rowKey={(r) => `${r.domain}-${r.server_id}-${r.server_name}`}
        />
      </Card>
    </>
  );
}
