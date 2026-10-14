import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 开发时由 TAURI_DEV_HOST 指定宿主机（Android 真机调试用）
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  // Tauri 需要看到完整的编译输出
  clearScreen: false,
  server: {
    port: 1420,
    // 端口被占用时直接失败，而不是自动换端口（否则 Tauri 找不到前端）
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // src-tauri 由 cargo 监听，Vite 不必扫
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // Windows 用 WebView2（Chromium），Linux/macOS 用 WebKit
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
