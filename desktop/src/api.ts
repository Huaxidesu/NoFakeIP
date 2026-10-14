// 唯一的 IPC 出口：所有 Tauri command 调用都集中在这里。
// 页面组件**不允许**直接 import @tauri-apps/api（见 doc/10-开发规范.md 第 3 节）。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppStatus,
  BenchOptions,
  BenchProgress,
  Classification,
  DataProgress,
  DataStatus,
  DnsServer,
  DomainBenchResult,
  DomainDiagnosis,
  DomainHistoryRow,
  FullBenchReport,
  Recommendation,
  Settings,
  UpdateReport,
} from "./types";

/** 统一把 Rust 的 Err(String) 转成 Error。 */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : String(e));
  }
}

// —— 状态 ——

export const getStatus = () => call<AppStatus>("get_status");
export const getSettings = () => call<Settings>("get_settings");
export const setSettings = (settings: Settings) =>
  call<void>("set_settings", { settings });
export const getDataDir = () => call<string>("get_data_dir");
export const getRecommendation = () => call<Recommendation | null>("get_recommendation");

// —— 测速与诊断 ——

export const runBenchmark = (domains: string[], serverIds: number[], opts: BenchOptions) =>
  call<DomainBenchResult[]>("run_benchmark", { domains, serverIds, opts });

export const runFullBenchmark = (opts: BenchOptions) =>
  call<FullBenchReport>("run_full_benchmark", { opts });

export const classifyDomain = (domain: string) =>
  call<Classification>("classify_domain", { domain });

export const detectDomain = (domain: string, serverIds: number[]) =>
  call<DomainDiagnosis>("detect_domain", { domain, serverIds });

// —— 报告 ——

/** scope: "domain" | "last"；format: "md" | "json" | "csv" */
export const generateReport = (scope: string, domain: string | null, format: string) =>
  call<string>("generate_report", { scope, domain, format });

export const saveTextFile = (content: string, path: string) =>
  call<void>("save_text_file", { content, path });

// —— 系统文件对话框（统一在这里 import 插件，页面组件不直接依赖 @tauri-apps/*）——

export async function pickSavePath(defaultName: string, extensions: string[]): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: extensions.join("/").toUpperCase(), extensions }],
  });
  return path ?? null;
}

export async function pickOpenPath(extensions: string[]): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const path = await open({
    multiple: false,
    directory: false,
    filters: extensions.length ? [{ name: extensions.join("/").toUpperCase(), extensions }] : undefined,
  });
  return typeof path === "string" ? path : null;
}

// —— 规则数据 ——

export const listDataSources = () => call<DataStatus[]>("list_data_sources");
export const updateDataSources = (ids: string[]) =>
  call<UpdateReport[]>("update_data_sources", { ids });

// —— DNS 服务器 ——

export const listServers = (onlyEnabled: boolean) =>
  call<DnsServer[]>("list_servers", { onlyEnabled });
export const addServer = (spec: string, name: string | null, region: string | null) =>
  call<DnsServer>("add_server", { spec, name, region });
export const updateServer = (server: DnsServer) =>
  call<void>("update_server", { server });
export const deleteServer = (id: number) => call<void>("delete_server", { id });
export const setServerEnabled = (id: number, enabled: boolean) =>
  call<void>("set_server_enabled", { id, enabled });
export const importServers = (path: string, format: string) =>
  call<number>("import_servers", { path, format });
export const exportServers = (path: string, format: string) =>
  call<void>("export_servers", { path, format });

// —— 历史 ——

export const listHistory = (limit: number) =>
  call<DomainHistoryRow[]>("list_history", { limit });
export const listBenchmarks = (domain: string | null, limit: number) =>
  call<DomainBenchResult[]>("list_benchmarks", { domain, limit });

// —— 事件（长任务进度）——

export function onBenchProgress(cb: (p: BenchProgress) => void): Promise<UnlistenFn> {
  return listen<BenchProgress>("bench:progress", (e) => cb(e.payload));
}

export function onDataProgress(cb: (p: DataProgress) => void): Promise<UnlistenFn> {
  return listen<DataProgress>("data:progress", (e) => cb(e.payload));
}
