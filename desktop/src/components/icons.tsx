// 导航与操作图标：一律用**内联 SVG**，不用字体字形。
//
// 为什么（2026-10-15 实测踩坑）：原先用 ⚡(U+26A1) / ⚙(U+2699) 这类字符当图标。
// 它们是 **emoji 码位**，Android 会渲染成彩色表情；补 U+FE0E 变体选择符
// 也压不住 —— 在 MuMu（Android 12）上实测闪电仍是黄色。
// 字体字形还依赖系统字体覆盖，容易出现风格不统一甚至缺字（豆腐块）。
//
// 改用 SVG 后：颜色完全由 CSS（currentColor）控制，各图标统一 1.8 描边、
// 圆角端点，风格天然一致。

import type { ReactNode } from "react";

export type IconName =
  | "dashboard"
  | "bolt"
  | "target"
  | "doc"
  | "clock"
  | "sliders"
  | "info"
  | "more";

const PATHS: Record<IconName, ReactNode> = {
  // 首页：四宫格
  dashboard: (
    <>
      <rect x="3" y="3" width="7.5" height="7.5" rx="1.6" />
      <rect x="13.5" y="3" width="7.5" height="7.5" rx="1.6" />
      <rect x="3" y="13.5" width="7.5" height="7.5" rx="1.6" />
      <rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.6" />
    </>
  ),
  // DNS 测速：闪电
  bolt: <path d="M13.2 2.2 4.6 13.4h5.8l-1.1 8.4 8.9-11.6h-5.9z" />,
  // 域名检测：靶心
  target: (
    <>
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="3.4" />
    </>
  ),
  // 推荐与报告：文档
  doc: (
    <>
      <rect x="4" y="3" width="16" height="18" rx="2.2" />
      <path d="M8 8.5h8M8 12.5h8M8 16.5h5" />
    </>
  ),
  // 历史记录：时钟
  clock: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 6.8V12l3.6 2.2" />
    </>
  ),
  // 设置：滑杆
  sliders: (
    <>
      <path d="M4 7h8M16.5 7H20M4 12h3.5M12 12h8M4 17h8M16.5 17H20" />
      <circle cx="14.2" cy="7" r="2.3" />
      <circle cx="9.7" cy="12" r="2.3" />
      <circle cx="14.2" cy="17" r="2.3" />
    </>
  ),
  // 关于：信息
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5.2" />
      <circle cx="12" cy="7.6" r="0.9" fill="currentColor" stroke="none" />
    </>
  ),
  // 更多：三点
  more: (
    <>
      <circle cx="5.2" cy="12" r="1.7" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1.7" fill="currentColor" stroke="none" />
      <circle cx="18.8" cy="12" r="1.7" fill="currentColor" stroke="none" />
    </>
  ),
};

/** 统一规格的线性图标；颜色继承 `currentColor`。 */
export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}
