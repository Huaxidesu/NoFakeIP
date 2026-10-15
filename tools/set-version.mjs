#!/usr/bin/env node
/**
 * 一键同步版本号。
 *
 * 为什么要这个脚本：版本号散落在三个文件里 ——
 *   1. Cargo.toml            [workspace.package] version   ← 唯一事实来源
 *   2. desktop/package.json  version
 *   3. desktop/src-tauri/tauri.conf.json  version
 *   （desktop/src-tauri/Cargo.toml 用的是字面量，也一并同步）
 *
 * 手工改这三个地方很容易漏掉一个，然后发出的包版本号跟 tag 对不上。
 * 发布流水线（.github/workflows/release.yml）在构建前会校验三者一致，
 * 不一致直接失败，所以这里提供同步工具。
 *
 * 用法：
 *   node tools/set-version.mjs            # 只检查，打印当前版本与一致性
 *   node tools/set-version.mjs 0.2.0      # 把三处都改成 0.2.0
 *   node tools/set-version.mjs --from-cargo   # 以 Cargo.toml 为准，同步到其余文件
 */

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

const FILES = {
  cargo: join(ROOT, "Cargo.toml"),
  pkg: join(ROOT, "desktop", "package.json"),
  tauriConf: join(ROOT, "desktop", "src-tauri", "tauri.conf.json"),
  tauriCargo: join(ROOT, "desktop", "src-tauri", "Cargo.toml"),
};

/** 从 Cargo.toml 的 [workspace.package] 段读 version */
function readCargoVersion() {
  const text = readFileSync(FILES.cargo, "utf8");
  const m = text.match(/\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/);
  if (!m) throw new Error("无法在 Cargo.toml 的 [workspace.package] 中找到 version");
  return m[1];
}

/** 读取 tauri.conf.json（普通 JSON，可以直接 parse） */
function readTauriConfVersion() {
  return JSON.parse(readFileSync(FILES.tauriConf, "utf8")).version;
}

/** 读取 src-tauri/Cargo.toml 的 [package] version */
function readTauriCargoVersion() {
  const text = readFileSync(FILES.tauriCargo, "utf8");
  const m = text.match(/\[package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/);
  if (!m) throw new Error("无法在 desktop/src-tauri/Cargo.toml 的 [package] 中找到 version");
  return m[1];
}

function readPkgVersion() {
  return JSON.parse(readFileSync(FILES.pkg, "utf8")).version;
}

/* ------------------------------------------------------------------ 写入 */

/** 只替换 [section] 段内的 version，避免误改依赖的版本号 */
function writeCargoVersion(path, section, version) {
  const text = readFileSync(path, "utf8");
  const re = new RegExp(`(\\[${section.replace(/\./g, "\\.")}\\][\\s\\S]*?\\nversion\\s*=\\s*)"[^"]+"`);
  if (!re.test(text)) throw new Error(`${path} 中找不到 [${section}] 段或其 version`);
  writeFileSync(path, text.replace(re, `$1"${version}"`), "utf8");
}

function writeJsonVersion(path, version) {
  // 刻意不用 JSON.parse + JSON.stringify 往返：那两个文件里存在
  // 紧凑的单行数组（如 "targets": ["nsis"]），stringify 会把它们展开成多行，
  // 每次改版本号都会带出一大片无意义的格式 diff。
  // 这里只替换顶层那一个 `"version": "x.y.z",` 行，其余字节原样保留。
  const text = readFileSync(path, "utf8");
  // 顶层 version 一定是第 2 个键（前面是 $schema / productName），
  // 用「行首两个空格」锚定顶层缩进，避免误伤嵌套对象里的 version。
  const re = /^(\s{2}"version"\s*:\s*)"[^"]*"/m;
  if (!re.test(text)) throw new Error(`${path} 中找不到顶层 version 字段`);
  const next = text.replace(re, `$1"${version}"`);
  if (next !== text) writeFileSync(path, next, "utf8");
}

/* ------------------------------------------------------------------ 主流程 */

const SEMVER = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;

function current() {
  return {
    cargo: readCargoVersion(),
    pkg: readPkgVersion(),
    tauriConf: readTauriConfVersion(),
    tauriCargo: readTauriCargoVersion(),
  };
}

function report(v) {
  const rows = [
    ["Cargo.toml (workspace)", v.cargo, true],
    ["desktop/package.json", v.pkg, false],
    ["desktop/src-tauri/tauri.conf.json", v.tauriConf, false],
    ["desktop/src-tauri/Cargo.toml", v.tauriCargo, false],
  ];
  console.log("\n当前版本号：");
  for (const [name, ver, isSource] of rows) {
    const mark = ver === v.cargo ? "✅" : "❌";
    console.log(`  ${mark} ${name.padEnd(36)} ${ver}${isSource ? "   ← 事实来源" : ""}`);
  }
  const allSame = new Set(Object.values(v)).size === 1;
  console.log(allSame ? "\n✅ 四处一致\n" : "\n❌ 存在不一致，运行 node tools/set-version.mjs <版本> 同步\n");
  return allSame;
}

const arg = process.argv[2];
const v0 = current();

if (!arg) {
  process.exit(report(v0) ? 0 : 1);
}

if (arg === "--from-cargo") {
  const target = v0.cargo;
  console.log(`以 Cargo.toml 为准（${target}）同步其余文件…`);
  writeJsonVersion(FILES.pkg, target);
  writeJsonVersion(FILES.tauriConf, target);
  writeCargoVersion(FILES.tauriCargo, "package", target);
  report(current());
  console.log(`已同步到 ${target}`);
  process.exit(0);
}

if (!SEMVER.test(arg)) {
  console.error(`❌ 版本号格式不合法：${arg}`);
  console.error("   应为 semver，例如 0.2.0 / 1.0.0-rc.1");
  process.exit(1);
}

console.log(`将版本号改为 ${arg}（原 ${v0.cargo}）：`);
writeCargoVersion(FILES.cargo, "workspace.package", arg);
writeJsonVersion(FILES.pkg, arg);
writeJsonVersion(FILES.tauriConf, arg);
writeCargoVersion(FILES.tauriCargo, "package", arg);
report(current());
console.log(`已同步到 ${arg}`);
console.log("提示：若 desktop/Cargo.lock 或根 Cargo.lock 记录了本包版本，");
console.log("      下次 cargo build 时会自动更新，无需手动改。");
