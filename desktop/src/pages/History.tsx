// 历史记录页：域名历史表 + 选中域名的延迟趋势图。

import { useEffect, useMemo, useState } from "react";
import type { AppCtx } from "../App";
import * as api from "../api";
import { EChart, themeColors } from "../components/EChart";
import { Card, DataTable, RegionBadge, RiskBadge, useToast, type Column } from "../components/ui";
import type { DomainBenchResult, DomainHistoryRow } from "../types";

export default function History(_props: { ctx: AppCtx }) {
  const toast = useToast();
  const [rows, setRows] = useState<DomainHistoryRow[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [trend, setTrend] = useState<DomainBenchResult[]>([]);
  const [loadingTrend, setLoadingTrend] = useState(false);

  const load = async () => {
    try {
      setRows(await api.listHistory(200));
    } catch (e) {
      toast(`读取历史失败：${(e as Error).message}`, "error");
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const openTrend = async (domain: string) => {
    setSelected(domain);
    setLoadingTrend(true);
    try {
      setTrend(await api.listBenchmarks(domain, 100));
    } catch (e) {
      toast(`读取趋势失败：${(e as Error).message}`, "error");
    } finally {
      setLoadingTrend(false);
    }
  };

  const columns: Column<DomainHistoryRow>[] = [
    {
      key: "domain",
      title: "域名",
      render: (r) => (
        <button className="btn btn-sm" onClick={() => void openTrend(r.domain)}>
          {r.domain}
        </button>
      ),
    },
    { key: "region", title: "归属", render: (r) => <RegionBadge region={r.region} /> },
    {
      key: "risk",
      title: "风险",
      render: (r) => (r.risk ? <RiskBadge risk={r.risk} /> : <span className="hint">—</span>),
      sortValue: (r) => r.risk ?? "",
    },
    { key: "fake", title: "Fake-IP", render: (r) => <span className="hint">{r.fake_ip ?? "—"}</span> },
    {
      key: "ips",
      title: "最近解析",
      render: (r) => <span className="mono hint">{r.last_ips.join(", ") || "—"}</span>,
    },
    {
      key: "count",
      title: "查询次数",
      align: "right",
      render: (r) => r.query_count,
      sortValue: (r) => r.query_count,
    },
    {
      key: "time",
      title: "最近时间",
      render: (r) => <span className="hint">{new Date(r.last_seen_at).toLocaleString()}</span>,
      sortValue: (r) => r.last_seen_at,
    },
  ];

  // 趋势图：X = 时间，每个服务器一条线
  const chartOption = useMemo(() => {
    const c = themeColors();
    const times = [...new Set(trend.map((t) => t.created_at))].sort();
    const servers = [...new Set(trend.flatMap((t) => t.results.map((r) => r.server_name)))];
    const palette = [c.accent, c.secondary, c.ok, c.warn, c.danger, "#0ea5e9", "#ec4899", "#14b8a6", "#f97316"];
    const series = servers.map((name, i) => ({
      name,
      type: "line" as const,
      smooth: true,
      symbolSize: 5,
      connectNulls: true,
      itemStyle: { color: palette[i % palette.length] },
      data: times.map((t) => {
        const hit = trend.find((x) => x.created_at === t)?.results.find((r) => r.server_name === name);
        return hit && hit.ok ? Number(hit.avg_ms.toFixed(1)) : null;
      }),
    }));
    return {
      grid: { left: 8, right: 16, top: 34, bottom: 8, containLabel: true },
      tooltip: { trigger: "axis", valueFormatter: (v: number) => (v == null ? "失败" : `${v} ms`) },
      legend: { textStyle: { color: c.fgMuted }, top: 0, type: "scroll" },
      xAxis: {
        type: "category",
        data: times.map((t) => new Date(t).toLocaleString()),
        axisLabel: { color: c.fgMuted, hideOverlap: true, fontSize: 10 },
        axisLine: { lineStyle: { color: c.border } },
      },
      yAxis: {
        type: "value",
        name: "ms",
        nameTextStyle: { color: c.fgMuted },
        axisLabel: { color: c.fgMuted },
        splitLine: { lineStyle: { color: c.border } },
      },
      series,
    };
  }, [trend]);

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">历史记录</h1>
          <p className="page-desc">本机 SQLite 中保存的域名诊断历史。点击域名可查看延迟趋势。</p>
        </div>
        <div className="btn-row">
          <button className="btn" onClick={load}>
            刷新
          </button>
        </div>
      </div>

      <Card title={`共 ${rows.length} 个域名`}>
        <DataTable
          columns={columns}
          rows={rows}
          empty="还没有历史记录"
          initialSortKey="time"
          initialSortDesc
          rowKey={(r) => r.domain}
        />
      </Card>

      {selected && (
        <Card
          title={`${selected} · 延迟趋势`}
          actions={
            <button className="btn btn-sm" onClick={() => setSelected(null)}>
              关闭
            </button>
          }
        >
          {loadingTrend ? (
            <div className="empty">加载中…</div>
          ) : trend.length === 0 ? (
            <div className="empty">该域名还没有测速明细</div>
          ) : (
            <EChart option={chartOption} height={300} />
          )}
        </Card>
      )}
    </>
  );
}
