// 首页：主要操作入口 + 状态摘要 + 推荐结论。
//
// 设计取舍（2026-10-15，需求方要求「按钮加大、内容缩小到一页」）：
//   - 主要操作改成**大幅卡片**（桌面/窄屏都用同一套，窄屏自动竖排），
//     替代原来挤在标题右上角的小按钮 —— 那个位置又小又不显眼；
//   - 只保留「当前状态」与「推荐结论」两块摘要，数据库细节挪到「关于」页；
//   - 完整的历史记录表格挪到「历史记录」页，首页不再堆表格，保证一屏内看完。
//
// 长任务状态放在 tasks.ts 的模块级 store 里：切换页面不会丢进度与结果。

import { useEffect, useState } from "react";
import type { AppCtx } from "../App";
import * as api from "../api";
import { Card, ProgressBar, Spinner, Stat, useToast } from "../components/ui";
import { Icon } from "../components/icons";
import { startFullBench, useFullBench } from "../tasks";
import type { Recommendation } from "../types";

export default function Dashboard({
  ctx,
  onGoto,
}: {
  ctx: AppCtx;
  onGoto: (page: string) => void;
}) {
  const toast = useToast();
  const [fetchedRec, setFetchedRec] = useState<Recommendation | null>(null);
  const full = useFullBench();

  // 本次会话刚跑完的结论优先于启动时拉到的旧结论
  const rec = full.report?.recommendation ?? fetchedRec;

  useEffect(() => {
    let alive = true;
    api
      .getRecommendation()
      .then((r) => {
        if (alive) setFetchedRec(r);
      })
      .catch((e) => toast(`读取推荐结论失败：${(e as Error).message}`, "error"));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const running = full.running;
  const disabled = running || !ctx.settings;

  const runFullBench = () => {
    if (!ctx.settings) return;
    void startFullBench(
      {
        rounds: ctx.settings.rounds,
        timeout_secs: ctx.settings.timeout_secs,
        concurrency: ctx.settings.concurrency,
        domains: ctx.settings.sample_domains,
      },
      ctx.refreshStatus,
    );
  };

  const s = ctx.status;

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">首页</h1>
          <p className="page-desc">测出哪家 DNS 在国内快、哪家解析国外域名干净，并检查本地 Fake-IP。</p>
        </div>
      </div>

      {/* 主要操作：大幅卡片，桌面并排、窄屏竖排 */}
      <div className="quick-start">
        <button className="quick-tile quick-tile-primary" onClick={() => onGoto("check")}>
          <span className="quick-icon" aria-hidden="true">
            <Icon name="target" className="quick-icon-svg" />
          </span>
          <span className="quick-text">
            <span className="quick-title">域名检测</span>
            <span className="quick-desc">输入一个域名，看它解析干不干净、有没有被污染</span>
          </span>
          <span className="quick-arrow" aria-hidden="true">
            {"\u203A"}
          </span>
        </button>

        <button className="quick-tile" onClick={runFullBench} disabled={disabled}>
          <span className="quick-icon" aria-hidden="true">
            <Icon name="bolt" className="quick-icon-svg" />
          </span>
          <span className="quick-text">
            <span className="quick-title">{running ? "测速中…" : "开始全量测速"}</span>
            <span className="quick-desc">
              {running && full.progress
                ? `已完成 ${full.progress.done}/${full.progress.total}`
                : "把全部已启用节点跑一遍，生成推荐结论"}
            </span>
          </span>
          <span className="quick-arrow" aria-hidden="true">
            {running ? <Spinner /> : "\u203A"}
          </span>
        </button>
      </div>

      {running && full.progress && (
        <Card title="测速进度">
          <ProgressBar done={full.progress.done} total={full.progress.total} label={full.progress.current_domain} />
        </Card>
      )}

      <Card title="当前状态">
        <div className="grid grid-3">
          <Stat
            label="DNS 节点"
            value={`${s?.enabled_count ?? 0} / ${s?.server_count ?? 0}`}
            sub="已启用 / 全部"
          />
          <Stat label="已记录域名" value={s?.stats.domains ?? 0} sub={`测速记录 ${s?.stats.benchmarks ?? 0} 条`} />
          <Stat
            label="上次全量测速"
            value={s?.last_bench_at ? new Date(s.last_bench_at).toLocaleString() : "从未"}
            sub={rec ? `污染率 ${(rec.pollution_rate * 100).toFixed(0)}%` : "还没有推荐结论"}
          />
        </div>
      </Card>

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
            <div className="grid grid-2 rec-stats">
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
            <ul className="reasons">
              {rec.reasons.slice(0, 2).map((r, i) => (
                <li key={i}>{r}</li>
              ))}
            </ul>
          </>
        ) : (
          <div className="empty">还没有推荐结论，点上面的「开始全量测速」跑一次即可生成。</div>
        )}
      </Card>
    </>
  );
}
