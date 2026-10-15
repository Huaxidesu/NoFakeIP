// 「更多」页：窄屏底栏只放 4 个入口，其余页面从这里进。
// 桌面端不会用到本页（侧栏直接列出全部页面），保留它是为了窄屏可达性。

// 图标同 App.tsx：用单色文字符号，emoji 码位补 U+FE0E 强制文字呈现。
const ITEMS = [
  { key: "report", icon: "\u25A4", title: "推荐与报告", desc: "查看推荐结论、导出诊断报告" },
  { key: "history", icon: "\u25F7", title: "历史记录", desc: "按域名查看历史解析与测速结果" },
  { key: "settings", icon: "\u2699\uFE0E", title: "设置", desc: "测试参数、DNS 节点、规则数据" },
  { key: "about", icon: "\u24D8", title: "关于", desc: "版本与本机数据位置" },
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
            <span className="menu-icon">{it.icon}</span>
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
