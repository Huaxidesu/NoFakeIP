// 关于页：版本、简介与运行数据位置。
// 说明：本页不出现「不生成/不修改代理软件配置」这类表述（经用户确认删除）。

import type { AppCtx } from "../App";
import { Card, Stat } from "../components/ui";
import logo from "../assets/logo.png";

export default function About({ ctx }: { ctx: AppCtx }) {
  const s = ctx.status;

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">关于</h1>
          <p className="page-desc">版本信息与本机数据位置。</p>
        </div>
      </div>

      <Card>
        <div className="about-head">
          <img className="about-logo" src={logo} alt="NoFakeIP" />
          <div className="about-meta">
            <div className="about-name">NoFakeIP</div>
            <div className="about-version">v{s?.version ?? "—"} · DNS 诊断与测速</div>
          </div>
        </div>
        <p className="about-desc">
          NoFakeIP 帮你测出哪家 DNS 在国内快、哪家解析国外域名干净，并检查本地 Fake-IP 是否正常。
          延迟取自完整协议栈（UDP / TCP / DoT / DoH），不是 ping。
        </p>
      </Card>

      <Card title="运行数据">
        <div className="grid grid-4">
          <Stat label="DNS 节点" value={`${s?.enabled_count ?? 0} / ${s?.server_count ?? 0}`} sub="已启用 / 全部" />
          <Stat label="数据库" value={`schema v${s?.stats.schema_version ?? "-"}`} sub={`测速记录 ${s?.stats.benchmarks ?? 0} 条`} />
          <Stat label="已记录域名" value={s?.stats.domains ?? 0} sub={`规则条目 ${(s?.stats.rules ?? 0).toLocaleString()}`} />
          <Stat
            label="上次全量测速"
            value={s?.last_bench_at ? new Date(s.last_bench_at).toLocaleString() : "从未"}
            sub="可在首页发起"
          />
        </div>
        <div className="about-path">
          <div className="stat-label">数据目录</div>
          <div className="mono hint">{s?.data_dir ?? "—"}</div>
        </div>
        <p className="hint" style={{ marginTop: 12 }}>
          所有数据保存在本机，不会上传。
        </p>
      </Card>
    </>
  );
}
