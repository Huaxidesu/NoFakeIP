// 域名检测页：一个域名 → 分类 + 各 DNS 结果 + 污染/Fake-IP 判定 + 推荐。
//
// 检测状态与输入内容都在 tasks.ts 的模块级 store 里：切页面不会丢结果。

import type { AppCtx } from "../App";
import {
  Card,
  DataTable,
  FakeIpBadge,
  Findings,
  LatencyBadge,
  RegionBadge,
  RiskBadge,
  ServerResultCards,
  Spinner,
  Stat,
  type Column,
} from "../components/ui";
import { setDetectDomain, startDetect, useDetect } from "../tasks";
import type { ServerBenchResult } from "../types";
import { PROTOCOL_LABEL, SOURCE_LABEL } from "../types";

export default function DomainCheck({ ctx }: { ctx: AppCtx }) {
  const detect = useDetect();
  const { domainText: domain, loading, diag } = detect;

  const run = () => {
    const d = domain.trim();
    if (!d) return;
    const enabled = ctx.servers.filter((s) => s.enabled).map((s) => s.id);
    void startDetect(d, enabled, ctx.refreshStatus);
  };

  const columns: Column<ServerBenchResult>[] = [
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
      key: "ttl",
      title: "TTL",
      align: "right",
      render: (r) => (r.ttl != null ? r.ttl : "—"),
    },
    {
      key: "answers",
      title: "返回 IP",
      render: (r) => (
        <span className="mono hint">
          {r.answers.length ? r.answers.join(", ") : r.error ?? "—"}
        </span>
      ),
    },
  ];

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">域名检测</h1>
          <p className="page-desc">
            判断域名归属、交叉比对多个 DNS 的解析结果，识别污染与本地 Fake-IP 状态。
          </p>
        </div>
        <div className="btn-row">
          <button className="btn btn-primary" onClick={run} disabled={loading}>
            {loading ? (
              <>
                <Spinner /> 检测中…
              </>
            ) : (
              "开始检测"
            )}
          </button>
        </div>
      </div>

      <Card>
        <div className="row">
          <input
            type="text"
            className="grow"
            placeholder="例如 github.com"
            value={domain}
            onChange={(e) => setDetectDomain(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") run();
            }}
          />
          <span className="hint">
            将使用 {ctx.servers.filter((s) => s.enabled).length} 个已启用节点做交叉比对
          </span>
        </div>
      </Card>

      {diag && (
        <>
          <div className="grid grid-4">
            <Card>
              <Stat
                label="归属"
                value={<RegionBadge region={diag.classification.region} />}
                sub={`置信度 ${diag.classification.confidence.toFixed(2)} · ${SOURCE_LABEL[diag.classification.source]}`}
              />
            </Card>
            <Card>
              <Stat label="污染风险" value={<RiskBadge risk={diag.report.risk} />} sub={`${diag.report.findings.length} 条发现`} />
            </Card>
            <Card>
              <Stat
                label="Fake-IP"
                value={<FakeIpBadge status={diag.report.fake_ip} />}
                sub={
                  diag.report.fake_ip_hits.length
                    ? diag.report.fake_ip_hits.join(", ")
                    : "未发现 Fake-IP 应答"
                }
              />
            </Card>
            <Card>
              <Stat
                label="一致性"
                value={
                  diag.report.consistency != null
                    ? `${(diag.report.consistency * 100).toFixed(0)}%`
                    : "—"
                }
                sub={`成功 ${diag.bench.results.filter((r) => r.ok).length}/${diag.bench.results.length} 台`}
              />
            </Card>
          </div>

          <Card title="DNS 测速结果">
            {/* 宽屏：表格；窄屏：卡片（表格 6 列在手机上要左右滑） */}
            <div className="desktop-only">
              <DataTable
                columns={columns}
                rows={diag.bench.results}
                initialSortKey="latency"
                rowKey={(r) => `${r.server_id}-${r.server_name}`}
              />
            </div>
            <div className="mobile-only">
              <ServerResultCards rows={diag.bench.results} />
            </div>
          </Card>

          <div className="grid grid-2">
            <Card title="检测发现">
              <Findings findings={diag.report.findings} />
            </Card>
            <Card title="推荐">
              {diag.recommendation.china || diag.recommendation.foreign ? (
                <>
                  {diag.recommendation.china && (
                    <Stat
                      label="国内节点"
                      value={diag.recommendation.china.name}
                      sub={`${diag.recommendation.china.avg_ms.toFixed(0)}ms`}
                    />
                  )}
                  {diag.recommendation.foreign && (
                    <Stat
                      label="国外节点"
                      value={diag.recommendation.foreign.name}
                      sub={`${diag.recommendation.foreign.avg_ms.toFixed(0)}ms`}
                    />
                  )}
                </>
              ) : (
                <div className="empty">无可用推荐</div>
              )}
              <ul className="reasons" style={{ marginTop: 12 }}>
                {diag.recommendation.reasons.map((r, i) => (
                  <li key={i}>{r}</li>
                ))}
              </ul>
            </Card>
          </div>

          <Card title="分类详情">
            <DataTable
              columns={[
                { key: "k", title: "项", render: (r: { k: string; v: string }) => r.k },
                { key: "v", title: "值", render: (r: { k: string; v: string }) => <span className="mono">{r.v}</span> },
              ]}
              rows={[
                { k: "域名", v: diag.classification.domain },
                { k: "可注册域名", v: diag.classification.registrable ?? "—" },
                { k: "顶级域", v: diag.classification.tld ?? "—" },
                { k: "命中规则", v: diag.classification.matched_rule ?? "—" },
                { k: "判定来源", v: SOURCE_LABEL[diag.classification.source] },
              ]}
              rowKey={(r) => r.k}
            />
          </Card>
        </>
      )}

      {!diag && (
        <Card>
          <div className="empty">输入域名后点击「开始检测」，结果会显示在这里。</div>
        </Card>
      )}
    </>
  );
}
