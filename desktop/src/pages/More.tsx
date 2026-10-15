// 「更多」页：窄屏底栏只放 4 个入口，其余页面从这里进。
// 桌面端不会用到本页（侧栏直接列出全部页面），保留它是为了窄屏可达性。

import { Icon, type IconName } from "../components/icons";

// 图标与 App.tsx 保持一致：统一用内联 SVG（不用字体字形 / emoji 码位）。
const ITEMS: { key: string; icon: IconName; title: string; desc: string }[] = [
  { key: "report", icon: "doc", title: "推荐与报告", desc: "查看推荐结论、导出诊断报告" },
  { key: "history", icon: "clock", title: "历史记录", desc: "按域名查看历史解析与测速结果" },
  { key: "settings", icon: "sliders", title: "设置", desc: "测试参数、DNS 节点、规则数据" },
  { key: "about", icon: "info", title: "关于", desc: "版本与本机数据位置" },
];

export default function More({ onGoto }: { onGoto: (page: string) => void }) {
  return (
    <>
      <div className="page-head">
        <div>
          <h1 className="page-title">更多</h1>
          <p className="page-desc">报告、历史、设置与关于。</p>
        </div>
      </div>

      <div className="menu-list">
        {ITEMS.map((it) => (
          <button key={it.key} className="menu-item" onClick={() => onGoto(it.key)}>
            <span className="menu-icon">
              <Icon name={it.icon} className="menu-icon-svg" />
            </span>
            <span className="menu-text">
              <span className="menu-title">{it.title}</span>
              <span className="menu-desc">{it.desc}</span>
            </span>
            <span className="menu-arrow" aria-hidden="true">
              ›
            </span>
          </button>
        ))}
      </div>
    </>
  );
}
