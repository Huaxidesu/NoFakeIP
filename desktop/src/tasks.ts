// 长任务的状态与执行，统一放在**模块级 store** 里，而不是页面组件内。
//
// 为什么（bug 修复，2026-10-15）：
//   原先任务的 running / 进度 / 结果都存在页面组件的 useState 里。
//   页面一切换，组件卸载，这些状态连同结果一起丢掉 ——
//   用户看到的是「任务跑一半切走，回来变成空白页」，像是任务被中止了。
//   （实际 Rust 侧一直在跑：后端没有任何取消逻辑，只是前端把结果弄丢了。）
//
//   放到模块级 store 后：
//     · 任务的推进与结果不再依赖页面是否挂载；
//     · 切回来能看到实时进度 / 已完成的结果；
//     · 完成时用 notify() 弹全局提示，人在哪个页面都能看到。

import { useSyncExternalStore } from "react";
import * as api from "./api";
import { notify } from "./components/ui";
import type {
  BenchOptions,
  BenchProgress,
  DomainBenchResult,
  DomainDiagnosis,
  FullBenchReport,
} from "./types";

// —— 极简 store ——

type Listener = () => void;

function createStore<T extends object>(initial: T) {
  let state = initial;
  const listeners = new Set<Listener>();
  return {
    getState: () => state,
    setState(patch: Partial<T>) {
      state = { ...state, ...patch };
      listeners.forEach((l) => l());
    },
    subscribe(l: Listener) {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
  };
}

type Store<T extends object> = ReturnType<typeof createStore<T>>;

function useStore<T extends object>(store: Store<T>): T {
  return useSyncExternalStore(store.subscribe, store.getState, store.getState);
}

/** 把 Error / 字符串统一成可读消息 */
function msg(e: unknown): string {
  return typeof e === "string" ? e : String((e as Error)?.message ?? e);
}

// —— 全量测速（首页）——

export interface FullBenchState {
  running: boolean;
  progress: BenchProgress | null;
  report: FullBenchReport | null;
  error: string | null;
}

const fullBenchStore = createStore<FullBenchState>({
  running: false,
  progress: null,
  report: null,
  error: null,
});

export const useFullBench = () => useStore(fullBenchStore);

/** 启动全量测速。重复点击会被忽略（已在跑时直接返回）。 */
export async function startFullBench(opts: BenchOptions, onFinished?: () => void | Promise<void>) {
  if (fullBenchStore.getState().running) return;
  fullBenchStore.setState({
    running: true,
    error: null,
    progress: { done: 0, total: opts.domains?.length ?? 0, current_domain: "" },
  });

  let unlisten: (() => void) | undefined;
  try {
    unlisten = await api.onBenchProgress((p) => fullBenchStore.setState({ progress: p }));
    const report = await api.runFullBenchmark(opts);
    fullBenchStore.setState({ report });
    notify(
      `全量测速完成：${report.results.length} 个域名，用时 ${(report.elapsed_ms / 1000).toFixed(1)}s`,
      "ok",
    );
    await onFinished?.();
  } catch (e) {
    const m = msg(e);
    fullBenchStore.setState({ error: m });
    notify(`测速失败：${m}`, "error");
  } finally {
    unlisten?.();
    fullBenchStore.setState({ running: false, progress: null });
  }
}

// —— DNS 测速（测速页）——

export interface BenchState {
  running: boolean;
  results: DomainBenchResult[];
  error: string | null;
  // 表单内容也一起存：切页面回来时用户输入的东西还在
  domainsText: string;
  selected: number[] | null;
  rounds: number | null;
  timeoutSecs: number | null;
}

const benchStore = createStore<BenchState>({
  running: false,
  results: [],
  error: null,
  domainsText: "www.baidu.com, github.com",
  selected: null,
  rounds: null,
  timeoutSecs: null,
});

export const useBench = () => useStore(benchStore);

/** 更新测速页的表单状态（持久化在 store 里，切页面不丢） */
export function updateBenchForm(patch: Partial<Pick<BenchState, "domainsText" | "selected" | "rounds" | "timeoutSecs">>) {
  benchStore.setState(patch);
}

export async function startBench(
  domains: string[],
  serverIds: number[],
  opts: BenchOptions,
  onFinished?: () => void | Promise<void>,
) {
  if (benchStore.getState().running) return;
  benchStore.setState({ running: true, error: null });

  try {
    const r = await api.runBenchmark(domains, serverIds, opts);
    benchStore.setState({ results: r });
    const ok = r.reduce((n, d) => n + d.results.filter((s) => s.ok).length, 0);
    notify(`测速完成：${r.length} 个域名，${ok} 条成功结果`, "ok");
    await onFinished?.();
  } catch (e) {
    const m = msg(e);
    benchStore.setState({ error: m });
    notify(`测速失败：${m}`, "error");
  } finally {
    benchStore.setState({ running: false });
  }
}

// —— 域名检测（检测页）——

export interface DetectState {
  loading: boolean;
  diag: DomainDiagnosis | null;
  error: string | null;
  /** 输入框内容也存下来，切页面回来不丢 */
  domainText: string;
}

const detectStore = createStore<DetectState>({
  loading: false,
  diag: null,
  error: null,
  domainText: "github.com",
});

export const useDetect = () => useStore(detectStore);

export function setDetectDomain(text: string) {
  detectStore.setState({ domainText: text });
}

export async function startDetect(
  domain: string,
  serverIds: number[],
  onFinished?: () => void | Promise<void>,
) {
  if (detectStore.getState().loading) return;
  detectStore.setState({ loading: true, error: null });

  try {
    const r = await api.detectDomain(domain, serverIds);
    detectStore.setState({ diag: r });
    await onFinished?.();
  } catch (e) {
    const m = msg(e);
    detectStore.setState({ error: m });
    notify(`检测失败：${m}`, "error");
  } finally {
    detectStore.setState({ loading: false });
  }
}

// —— 全局「有任务在跑」指示 ——

/** 返回当前正在跑的任务描述；没有则返回 null。 */
export function useRunningTaskLabel(): string | null {
  const full = useFullBench();
  const bench = useBench();
  const detect = useDetect();

  if (full.running) {
    return full.progress && full.progress.total > 0
      ? `全量测速中 ${full.progress.done}/${full.progress.total}`
      : "全量测速中…";
  }
  if (bench.running) return "DNS 测速中…";
  if (detect.loading) return "域名检测中…";
  return null;
}
