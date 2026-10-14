// ECharts 轻量封装：按需引入 + 随主题变化重建。
// 不引 echarts-for-react（对 React 19 兼容性不确定），自己写 20 行更可控。

import { useEffect, useRef } from "react";
import * as echarts from "echarts/core";
import { BarChart, LineChart } from "echarts/charts";
import {
  GridComponent,
  LegendComponent,
  TooltipComponent,
  MarkLineComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import type { EChartsCoreOption } from "echarts/core";

echarts.use([
  BarChart,
  LineChart,
  GridComponent,
  LegendComponent,
  TooltipComponent,
  MarkLineComponent,
  CanvasRenderer,
]);

/** 读取当前主题的 CSS 变量，让图表跟随深/浅色。 */
export function themeColors() {
  const cs = getComputedStyle(document.documentElement);
  const get = (name: string, fallback: string) => cs.getPropertyValue(name).trim() || fallback;
  return {
    fg: get("--fg", "#1b1f24"),
    fgMuted: get("--fg-muted", "#5c6672"),
    border: get("--border", "#e3e6ea"),
    accent: get("--accent", "#3b82f6"),
    secondary: get("--secondary", "#8b5cf6"),
    ok: get("--ok", "#16a34a"),
    warn: get("--warn", "#f59e0b"),
    danger: get("--danger", "#dc2626"),
  };
}

export function EChart({
  option,
  height = 260,
}: {
  option: EChartsCoreOption;
  height?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  useEffect(() => {
    if (!ref.current) return;
    const chart = echarts.init(ref.current);
    chartRef.current = chart;
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    chartRef.current?.setOption(option, true);
  }, [option]);

  return <div ref={ref} style={{ width: "100%", height }} />;
}
