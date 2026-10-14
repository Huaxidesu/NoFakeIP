// 首页：当前状态 + 全量测速入口 + 最近结果 + 推荐结论。

import { useEffect, useState } from "react";
import type { AppCtx } from "../App";
import * as api from "../api";
import { Card, DataTable, ProgressBar, RegionBadge, RiskBadge, Stat, useToast, type Column } from "../components/ui";
import type { BenchProgress, DomainHistoryRow, Recommendation } from "../types";

export default function Dashboard({
  ctx,
  onGoto,
}: {
  ctx: AppCtx;
  onGoto: (page: string) => void;
}) {
  const toast = useToast();
  const [recent, setRecent] = useState<DomainHistoryRow[]>([]);
  const [rec, setRec] = useState<Recommendation | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<BenchProgress | null>(null);

  const load = async () => {
    try {
      const [h, r] = await Promise.all([api.listHistory(6), api.getRecommendation()]);
      setRecent(h);
      setRec(r);
    } catch (e) {
      toast(`读取历史失败：${(e as Error).message}`, "error");
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const startFullBench = async () => {
    if (!ctx.settings) return;
    setRunning(true);
    setProgress({ done: 0, total: ctx.settings.sample_domains.length, current_domain: "" });
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await api.onBenchProgress((p) => setProgress(p));
      const report = await api.runFullBenchmark({
        rounds: ctx.settings.rounds,
        timeout_secs: ctx.settings.timeout_secs,
        concurrency: ctx.settings.concurrency,
        domains: ctx.settings.sample_domains,
      });
      setRec(report.recommendation);
      toast(
        `全量测速完成：${report.results.length} 个域名，用时 ${(report.elapsed_ms / 1000).toFixed(1)}s`,
        "ok",
      );
      await Promise.all([load(), ctx.refreshStatus()]);
    } catch (e) {
      toast(`测速失败：${(e as Error).message}`, "error");
    } finally {
      unlisten?.();
      setRunning(false);
      setProgress(null);
    }
  };

  const columns: Column<DomainHistoryRow>[] = [
    { key: "domain", title: "域名", render: (r) => <span className="mono">{r.domain}</span> },
    { key: "region", title: "归属", render: (r) => <RegionBadge region={r.region} /> },
    {
      key: "risk",
      title: "风险",
      render: (r) => (r.risk ? <RiskBadge risk={r.risk} /> : <span className="hint">—</span>),
    },
    { key: "fake", title: "Fake-IP", render: (r) => <span className="hint">{r.fake_ip ?? "—"}</span> },
    {
      key: "ips",
      title: "最近解析",
      render: (r) => <span className="mono hint">{r.last_ips.join(", ") || "—"}</span>,
    },
    { key: "count", title: "次数", align: "right", render: (r) => r.query_count, sortValue: (r) => r.query_count },
  ];

  const s = ctx.status;

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">首页</h1>
          <p className="page-desc">
            NoFakeIP 帮你测出哪家 DNS 在国内快、哪家解析国外域名干净，并检查本地 Fake-IP 是否正常。
          </p>
        </div>
        <div className="btn-row">
          <button className="btn" onClick={() => onGoto("check")}>
            单域名检测
          </button>
          <button className="btn btn-primary" onClick={startFullBench} disabled={running || !ctx.settings}>
            {running ? "测速中…" : "开始全量测速"}
          </button>
        </div>
      </div>

      {running && progress && (
        <Card title="测速进度">
          <ProgressBar done={progress.done} total={progress.total} label={progress.current_domain} />
        </Card>
      )}

      <Card title="当前状态">
        <div className="grid grid-4">
          <Stat
            label="已启用 DNS 节点"
            value={`${s?.enabled_count ?? 0} / ${s?.server_count ?? 0}`}
            sub="可在「设置」中增删"
          />
          <Stat label="已记录域名" value={s?.stats.domains ?? 0} sub={`测速记录 ${s?.stats.benchmarks ?? 0} 条`} />
          <Stat
            label="数据库"
            value={`schema v${s?.stats.schema_version ?? "-"}`}
            sub={<span className="mono">{s?.data_dir ?? ""}</span>}
          />
          <Stat
            label="上次全量测速"
            value={s?.last_bench_at ? new Date(s.last_bench_at).toLocaleString() : "从未"}
            sub={rec ? `污染率 ${(rec.pollution_rate * 100).toFixed(0)}%` : "还没有推荐结论"}
          />
        </div>
      </Card>

      <div className="grid grid-2">
        <Card
          title="推荐结论"
          actions={
            <button className="btn btn-sm" onClick={() => onGoto("report")}>
              查看报告
            </button>
          }
        >
          {rec ? (
            <>
              <div className="grid grid-2" style={{ gap: 10 }}>
                <Stat
                  label="国内场景"
                  value={rec.china ? rec.china.name : "无可用"}
                  sub={rec.china ? `平均 ${rec.china.avg_ms.toFixed(0)}ms` : "请先做一次全量测速"}
                />
                <Stat
                  label="国外场景"
                  value={rec.foreign ? rec.foreign.name : "无可用"}
                  sub={rec.foreign ? `平均 ${rec.foreign.avg_ms.toFixed(0)}ms` : "请先做一次全量测速"}
                />
              </div>
              <ul className="reasons" style={{ marginTop: 12 }}>
                {rec.reasons.slice(0, 3).map((r, i) => (
                  <li key={i}>{r}</li>
                ))}
              </ul>
            </>
          ) : (
            <div className="empty">
              还没有推荐结论，点击右上角「开始全量测速」跑一次即可生成。
            </div>
          )}
        </Card>

        <Card title="说明">
          <ul className="reasons">
            <li>
              本工具只做<b>测量与判断</b>，不会生成或修改任何 Clash / OpenClash 配置。
            </li>
            <li>延迟来自完整协议栈（UDP / TCP / DoT / DoH），不是 ping。</li>
            <li>若检测到本机处于 Fake-IP 环境，明文 DNS 的结果会被本地劫持，建议看 DoH/DoT 的结果。</li>
            <li>所有数据保存在本机 SQLite，不会上传。</li>
          </ul>
        </Card>
      </div>

      <Card title="最近测试结果">
        <DataTable
          columns={columns}
          rows={recent}
          empty="还没有测试记录，点击「开始全量测速」或「单域名检测」开始"
          rowKey={(r) => r.domain}
        />
      </Card>
    </>
  );
}
