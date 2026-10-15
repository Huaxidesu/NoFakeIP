// 推荐与报告页：展示推荐结论，并生成 Markdown / JSON / CSV 报告。
// 注意：本页只输出诊断报告，不生成任何代理软件配置（见 doc/06-推荐与报告规格.md 第 4 节）。

import { useEffect, useState } from "react";
import type { AppCtx } from "../App";
import * as api from "../api";
import { Card, DataTable, RegionBadge, Spinner, Stat, useToast, type Column } from "../components/ui";
import type { Recommendation, RecommendedServer } from "../types";
import { PROTOCOL_LABEL } from "../types";

export default function Report(_props: { ctx: AppCtx }) {
  const toast = useToast();
  const [rec, setRec] = useState<Recommendation | null>(null);
  const [scope, setScope] = useState<"last" | "domain">("last");
  const [domain, setDomain] = useState("github.com");
  const [format, setFormat] = useState<"md" | "json" | "csv">("md");
  const [content, setContent] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        setRec(await api.getRecommendation());
      } catch (e) {
        toast(`读取推荐失败：${(e as Error).message}`, "error");
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const generate = async () => {
    setBusy(true);
    try {
      const text = await api.generateReport(scope, scope === "domain" ? domain.trim() : null, format);
      setContent(text);
      toast(`已生成 ${format.toUpperCase()} 报告（${text.length} 字符）`, "ok");
      setRec(await api.getRecommendation());
    } catch (e) {
      toast(`生成失败：${(e as Error).message}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!content) return;
    try {
      const ext = format === "md" ? "md" : format;
      const path = await api.pickSavePath(`nofakeip-report.${ext}`, [ext]);
      if (!path) return;
      await api.saveTextFile(content, path);
      toast(`已保存：${path}`, "ok");
    } catch (e) {
      toast(`保存失败：${(e as Error).message}`, "error");
    }
  };

  const copy = async () => {
    if (!content) return;
    try {
      await navigator.clipboard.writeText(content);
      toast("已复制到剪贴板", "ok");
    } catch {
      toast("复制失败，请手动选择文本", "warn");
    }
  };

  const altColumns: Column<RecommendedServer>[] = [
    { key: "name", title: "服务器", render: (r) => r.name },
    { key: "protocol", title: "协议", render: (r) => PROTOCOL_LABEL[r.protocol] },
    { key: "region", title: "区域", render: (r) => <RegionBadge region={r.region} /> },
    {
      key: "avg",
      title: "平均延迟",
      align: "right",
      render: (r) => `${r.avg_ms.toFixed(0)}ms`,
      sortValue: (r) => r.avg_ms,
    },
    {
      key: "rate",
      title: "成功率",
      align: "right",
      render: (r) => `${(r.success_rate * 100).toFixed(0)}%`,
      sortValue: (r) => r.success_rate,
    },
    { key: "score", title: "评分", align: "right", render: (r) => r.score.toFixed(0), sortValue: (r) => r.score },
  ];

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">推荐与报告</h1>
          <p className="page-desc">
            基于实测数据给出「国内用哪个、国外用哪个」的结论，并导出可读报告。
          </p>
        </div>
      </div>

      <Card title="推荐结论">
        {rec ? (
          <>
            <div className="grid grid-4">
              <Stat
                label="国内场景"
                value={rec.china ? rec.china.name : "无可用"}
                sub={rec.china ? `${rec.china.avg_ms.toFixed(0)}ms · ${PROTOCOL_LABEL[rec.china.protocol]}` : "请先跑一次测速"}
              />
              <Stat
                label="国外场景"
                value={rec.foreign ? rec.foreign.name : "无可用"}
                sub={rec.foreign ? `${rec.foreign.avg_ms.toFixed(0)}ms · ${PROTOCOL_LABEL[rec.foreign.protocol]}` : "请先跑一次测速"}
              />
              <Stat label="样本污染率" value={`${(rec.pollution_rate * 100).toFixed(0)}%`} sub="高风险域名占比" />
              <Stat label="生成时间" value={new Date(rec.generated_at).toLocaleString()} />
            </div>
            <ul className="reasons" style={{ marginTop: 14 }}>
              {rec.reasons.map((r, i) => (
                <li key={i}>{r}</li>
              ))}
            </ul>
          </>
        ) : (
          <div className="empty">还没有推荐结论。请先到「首页」执行全量测速，或到「DNS 测速」跑一批域名。</div>
        )}
      </Card>

      {rec && rec.alternates.length > 0 && (
        <Card title="备选节点">
          <DataTable columns={altColumns} rows={rec.alternates} initialSortKey="score" initialSortDesc rowKey={(r) => r.name} />
        </Card>
      )}

      <Card title="生成报告">
        <div className="row" style={{ gap: 16 }}>
          <div className="field" style={{ marginBottom: 0 }}>
            <label>范围</label>
            <select value={scope} onChange={(e) => setScope(e.target.value as "last" | "domain")}>
              <option value="last">最近一次全量测速（汇总报告）</option>
              <option value="domain">指定域名（单域名诊断报告）</option>
            </select>
          </div>
          {scope === "domain" && (
            <div className="field" style={{ marginBottom: 0 }}>
              <label>域名</label>
              <input type="text" value={domain} onChange={(e) => setDomain(e.target.value)} />
            </div>
          )}
          <div className="field" style={{ marginBottom: 0 }}>
            <label>格式</label>
            <select value={format} onChange={(e) => setFormat(e.target.value as "md" | "json" | "csv")}>
              <option value="md">Markdown</option>
              <option value="json">JSON</option>
              <option value="csv">CSV</option>
            </select>
          </div>
          <button className="btn btn-primary" onClick={generate} disabled={busy} style={{ marginTop: 18 }}>
            {busy ? (
              <>
                <Spinner /> 生成中…
              </>
            ) : (
              "生成报告"
            )}
          </button>
        </div>
        <p className="hint" style={{ marginTop: 10 }}>
          提示：本工具只输出诊断报告，不生成也不修改任何代理软件配置。
        </p>
      </Card>

      {content && (
        <Card
          title="报告预览"
          actions={
            <div className="btn-row">
              <button className="btn btn-sm" onClick={copy}>
                复制
              </button>
              <button className="btn btn-sm" onClick={save}>
                保存为文件
              </button>
            </div>
          }
        >
          <pre className="code">{content}</pre>
        </Card>
      )}
    </>
  );
}
