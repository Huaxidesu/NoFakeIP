// 设置页：通用设置 + DNS 节点管理 + 规则数据更新 + 关于。

import { useEffect, useState } from "react";
import type { AppCtx } from "../App";
import * as api from "../api";
import { Card, DataTable, Field, ProgressBar, RegionBadge, Spinner, useToast, type Column } from "../components/ui";
import type { DataProgress, DataStatus, DnsServer, Settings as SettingsModel } from "../types";
import { PROTOCOL_LABEL } from "../types";

export default function SettingsPage({ ctx }: { ctx: AppCtx }) {
  const toast = useToast();

  // —— 通用设置 ——
  const [form, setForm] = useState<SettingsModel | null>(ctx.settings);
  const [saving, setSaving] = useState(false);

  // —— 节点管理 ——
  const [spec, setSpec] = useState("");
  const [name, setName] = useState("");
  const [region, setRegion] = useState("china");

  // —— 数据管理 ——
  const [sources, setSources] = useState<DataStatus[]>([]);
  const [updating, setUpdating] = useState(false);
  const [dataProgress, setDataProgress] = useState<DataProgress | null>(null);

  useEffect(() => setForm(ctx.settings), [ctx.settings]);

  const loadSources = async () => {
    try {
      setSources(await api.listDataSources());
    } catch (e) {
      toast(`读取数据源失败：${(e as Error).message}`, "error");
    }
  };

  useEffect(() => {
    void loadSources();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveSettings = async () => {
    if (!form) return;
    setSaving(true);
    try {
      await api.setSettings(form);
      await ctx.refreshSettings();
      toast("设置已保存", "ok");
    } catch (e) {
      toast(`保存失败：${(e as Error).message}`, "error");
    } finally {
      setSaving(false);
    }
  };

  const addServer = async () => {
    if (!spec.trim()) {
      toast("请输入服务器地址", "warn");
      return;
    }
    try {
      const s = await api.addServer(spec.trim(), name.trim() || null, region);
      await ctx.refreshServers();
      await ctx.refreshStatus();
      setSpec("");
      setName("");
      toast(`已添加：${s.name}`, "ok");
    } catch (e) {
      toast(`添加失败：${(e as Error).message}`, "error");
    }
  };

  const toggleServer = async (s: DnsServer) => {
    try {
      await api.setServerEnabled(s.id, !s.enabled);
      await ctx.refreshServers();
      await ctx.refreshStatus();
    } catch (e) {
      toast(`操作失败：${(e as Error).message}`, "error");
    }
  };

  const removeServer = async (s: DnsServer) => {
    if (!window.confirm(`确定删除节点「${s.name}」？此操作不可撤销。`)) return;
    try {
      await api.deleteServer(s.id);
      await ctx.refreshServers();
      await ctx.refreshStatus();
      toast("已删除", "ok");
    } catch (e) {
      toast(`删除失败：${(e as Error).message}`, "error");
    }
  };

  const importServers = async () => {
    try {
      const path = await api.pickOpenPath(["json", "txt"]);
      if (!path) return;
      const fmt = path.toLowerCase().endsWith(".json") ? "json" : "txt";
      const n = await api.importServers(path, fmt);
      await ctx.refreshServers();
      toast(`已导入 ${n} 个节点`, "ok");
    } catch (e) {
      toast(`导入失败：${(e as Error).message}`, "error");
    }
  };

  const exportServers = async () => {
    try {
      const path = await api.pickSavePath("nofakeip-servers.txt", ["txt", "json"]);
      if (!path) return;
      const fmt = path.toLowerCase().endsWith(".json") ? "json" : "txt";
      await api.exportServers(path, fmt);
      toast(`已导出：${path}`, "ok");
    } catch (e) {
      toast(`导出失败：${(e as Error).message}`, "error");
    }
  };

  const updateData = async (ids: string[]) => {
    setUpdating(true);
    setDataProgress({ done: 0, total: ids.length || 2, name: "" });
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await api.onDataProgress((p) => setDataProgress(p));
      const reports = await api.updateDataSources(ids);
      const ok = reports.filter((r) => r.ok).length;
      if (ok === reports.length) toast(`全部更新成功（${ok} 项）`, "ok");
      else toast(`更新完成：成功 ${ok}/${reports.length}，失败项已保留原有数据`, "warn");
      await loadSources();
    } catch (e) {
      toast(`更新失败：${(e as Error).message}`, "error");
    } finally {
      unlisten?.();
      setUpdating(false);
      setDataProgress(null);
    }
  };

  const serverColumns: Column<DnsServer>[] = [
    { key: "id", title: "ID", align: "right", render: (s) => s.id },
    { key: "name", title: "名称", render: (s) => s.name },
    { key: "protocol", title: "协议", render: (s) => PROTOCOL_LABEL[s.protocol] },
    { key: "region", title: "区域", render: (s) => <RegionBadge region={s.region} /> },
    { key: "address", title: "地址", render: (s) => <span className="mono hint">{s.address}</span> },
    {
      key: "state",
      title: "状态",
      render: (s) => (
        <span className={`badge ${s.enabled ? "badge-low" : "badge-neutral"}`}>
          {s.enabled ? "启用" : "禁用"}
        </span>
      ),
      sortValue: (s) => (s.enabled ? 0 : 1),
    },
    {
      key: "ops",
      title: "操作",
      render: (s) => (
        <div className="btn-row">
          <button className="btn btn-sm" onClick={() => void toggleServer(s)}>
            {s.enabled ? "禁用" : "启用"}
          </button>
          <button className="btn btn-sm btn-danger" onClick={() => void removeServer(s)}>
            删除
          </button>
        </div>
      ),
    },
  ];

  const dataColumns: Column<DataStatus>[] = [
    { key: "name", title: "数据源", render: (d) => d.name },
    { key: "id", title: "ID", render: (d) => <span className="mono hint">{d.id}</span> },
    {
      key: "state",
      title: "状态",
      render: (d) => (
        <span className={`badge ${d.using_builtin ? "badge-neutral" : "badge-low"}`}>
          {d.using_builtin ? "内置数据" : "已更新"}
        </span>
      ),
    },
    { key: "entries", title: "条目数", align: "right", render: (d) => d.entries.toLocaleString(), sortValue: (d) => d.entries },
    {
      key: "time",
      title: "更新时间",
      render: (d) => <span className="hint">{d.modified_at ? new Date(d.modified_at).toLocaleString() : "—"}</span>,
    },
    {
      key: "op",
      title: "操作",
      render: (d) => (
        <button className="btn btn-sm" disabled={updating} onClick={() => void updateData([d.id])}>
          更新
        </button>
      ),
    },
  ];

  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">设置</h1>
          <p className="page-desc">测试参数、DNS 节点管理、规则数据更新。</p>
        </div>
      </div>

      <Card
        title="通用设置"
        actions={
          <button className="btn btn-sm btn-primary" onClick={saveSettings} disabled={saving || !form}>
            {saving ? <Spinner /> : "保存"}
          </button>
        }
      >
        {form ? (
          <>
            <div className="row" style={{ gap: 16, alignItems: "flex-start" }}>
              <Field label="单次查询超时（秒）">
                <input
                  type="number"
                  min={1}
                  max={30}
                  style={{ width: 100 }}
                  value={form.timeout_secs}
                  onChange={(e) => setForm({ ...form, timeout_secs: Number(e.target.value) })}
                />
              </Field>
              <Field label="每服务器轮次">
                <input
                  type="number"
                  min={1}
                  max={10}
                  style={{ width: 100 }}
                  value={form.rounds}
                  onChange={(e) => setForm({ ...form, rounds: Number(e.target.value) })}
                />
              </Field>
              <Field label="并发上限" hint="Android 建议 8">
                <input
                  type="number"
                  min={1}
                  max={64}
                  style={{ width: 100 }}
                  value={form.concurrency}
                  onChange={(e) => setForm({ ...form, concurrency: Number(e.target.value) })}
                />
              </Field>
              <Field label="Fake-IP 网段">
                <input
                  type="text"
                  style={{ width: 170 }}
                  value={form.fake_ip_range}
                  onChange={(e) => setForm({ ...form, fake_ip_range: e.target.value })}
                />
              </Field>
            </div>

            <label className="row" style={{ gap: 8, cursor: "pointer", marginBottom: 12 }}>
              <input
                type="checkbox"
                checked={form.online_geo}
                onChange={(e) => setForm({ ...form, online_geo: e.target.checked })}
              />
              <span>
                启用在线 IP 归属地查询
                <span className="hint">
                  {" "}
                  （默认关闭。开启后会把解析到的 IP 发送给第三方查询服务，请自行评估隐私影响）
                </span>
              </span>
            </label>

            <Field label="全量测速样本域名（每行一个）">
              <textarea
                rows={6}
                value={form.sample_domains.join("\n")}
                onChange={(e) =>
                  setForm({
                    ...form,
                    sample_domains: e.target.value.split("\n").map((s) => s.trim()).filter(Boolean),
                  })
                }
              />
            </Field>
          </>
        ) : (
          <div className="empty">加载中…</div>
        )}
      </Card>

      <Card
        title="DNS 节点管理"
        actions={
          <div className="btn-row">
            <button className="btn btn-sm" onClick={importServers}>
              导入
            </button>
            <button className="btn btn-sm" onClick={exportServers}>
              导出
            </button>
          </div>
        }
      >
        <div className="row" style={{ gap: 10, marginBottom: 14 }}>
          <input
            type="text"
            className="grow"
            placeholder="223.5.5.5 / tls://1.1.1.1:853#cloudflare-dns.com / https://dns.alidns.com/dns-query"
            value={spec}
            onChange={(e) => setSpec(e.target.value)}
          />
          <input type="text" placeholder="名称（可选）" style={{ width: 150 }} value={name} onChange={(e) => setName(e.target.value)} />
          <select value={region} onChange={(e) => setRegion(e.target.value)}>
            <option value="china">国内</option>
            <option value="global">国外</option>
            <option value="unknown">未知</option>
          </select>
          <button className="btn btn-primary" onClick={addServer}>
            添加
          </button>
        </div>
        <DataTable
          columns={serverColumns}
          rows={ctx.servers}
          empty="还没有节点"
          rowKey={(s) => s.id}
        />
      </Card>

      <Card
        title="规则数据"
        actions={
          <button className="btn btn-sm" disabled={updating} onClick={() => void updateData([])}>
            {updating ? <Spinner /> : "更新默认数据源"}
          </button>
        }
      >
        {dataProgress && (
          <div style={{ marginBottom: 12 }}>
            <ProgressBar done={dataProgress.done} total={dataProgress.total} label={dataProgress.name} />
          </div>
        )}
        <DataTable columns={dataColumns} rows={sources} empty="没有可用数据源" rowKey={(d) => d.id} />
        <p className="hint" style={{ marginTop: 10 }}>
          更新失败时会<b>保留</b>本地已有数据，不会做破坏性写入。数据源与镜像策略见 doc/14-数据源与更新.md。
        </p>
      </Card>

      <Card title="关于">
        <div className="row" style={{ gap: 24 }}>
          <div>
            <div className="stat-label">版本</div>
            <div className="mono">v{ctx.status?.version ?? "—"}</div>
          </div>
          <div className="grow">
            <div className="stat-label">数据目录</div>
            <div className="mono hint">{ctx.status?.data_dir ?? "—"}</div>
          </div>
          <div>
            <div className="stat-label">数据库</div>
            <div className="mono hint">
              schema v{ctx.status?.stats.schema_version ?? "-"} · 测速记录 {ctx.status?.stats.benchmarks ?? 0}
            </div>
          </div>
        </div>
        <p className="hint" style={{ marginTop: 12 }}>
          NoFakeIP 只做 DNS 测量与判断，不生成、不修改任何 Clash / OpenClash / Clash Verge 配置。
          所有数据保存在本机，不会上传。
        </p>
      </Card>
    </>
  );
}
