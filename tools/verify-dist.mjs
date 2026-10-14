#!/usr/bin/env node
// 校验前端构建产物是否完整（防止把残缺的 dist 打进安装包）。
//
// 背景（务必了解，见 doc/11 第 6.6 节）：
//   vite 打包前会 emptyDir('dist') 清空输出目录。本机该操作被替换成「走回收站」的
//   代理实现，偶发超时；一旦在删除阶段中断，dist 里就会**只剩一部分文件**。
//   而 dist/index.html 是新生成的，仍引用着已被删掉的主包 → 应用启动后 JS 404，
//   页面全空白（背景色还在，因为 CSS 恰好没被删）。
//   更坑的是：`tauri build` 若用 `--config '{"build":{"beforeBuildCommand":""}}'`
//   跳过前端构建，就会把这份残缺产物原样打进 exe / APK，而且构建**不会报错**。
//
// 用法：
//   node tools/verify-dist.mjs            # 校验 desktop/dist
//   在 tauri build / 打包 APK 之前跑一次；退出码非 0 表示产物不完整。
//
// 建议在 CI 里也跑一遍（见 .github/workflows/release.yml）。

import { readFileSync, existsSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// 可选参数：指定要校验的 dist 目录（默认 desktop/dist）。便于自测。
const distDir = process.argv[2] ? resolve(process.argv[2]) : join(root, "desktop", "dist");
const indexPath = join(distDir, "index.html");

function fail(msg) {
  console.error(`\n❌ dist 校验失败：${msg}\n`);
  process.exit(1);
}

if (!existsSync(indexPath)) {
  fail(`找不到 ${indexPath}，请先在 desktop/ 下执行 pnpm build`);
}

const html = readFileSync(indexPath, "utf8");

// 抓出 index.html 里所有 /assets/xxx 形式的引用（script / link 都在其中）
const refs = [...html.matchAll(/["'(](\/assets\/[^"')\s]+)["')]/g)].map((m) => m[1]);

if (refs.length === 0) {
  fail("index.html 里没有任何 /assets/ 引用，产物结构异常");
}

const missing = [];
let total = 0;
console.log(`检查 ${distDir} 完整性：`);
for (const ref of [...new Set(refs)]) {
  const file = join(distDir, ref.replace(/^\//, ""));
  if (!existsSync(file)) {
    missing.push(ref);
    console.log(`  ✗ ${ref}  缺失`);
  } else {
    const size = statSync(file).size;
    total += size;
    console.log(`  ✓ ${ref}  ${(size / 1024).toFixed(1)} KB`);
  }
}

if (missing.length > 0) {
  fail(
    `以下资源被 index.html 引用但文件不存在：\n    ${missing.join("\n    ")}\n` +
      `  这通常意味着上一次 vite build 的 emptyDir 阶段被中断了。\n` +
      `  处理：重新在 desktop/ 下执行 pnpm build，并再次运行本脚本确认。`,
  );
}

// 主包太小基本可以断定是残缺产物（本项目正常约 780 KB / gzip 260 KB）
const mainJs = refs.find((r) => r.endsWith(".js") && !r.includes("DlmLVRad"));
if (mainJs) {
  const size = statSync(join(distDir, mainJs.replace(/^\//, ""))).size;
  if (size < 100 * 1024) {
    fail(`主包 ${mainJs} 只有 ${(size / 1024).toFixed(1)} KB，明显不完整（正常约 780 KB）`);
  }
}

console.log(`\n✅ dist 校验通过（${[...new Set(refs)].length} 个资源，合计 ${(total / 1024).toFixed(1)} KB）\n`);
