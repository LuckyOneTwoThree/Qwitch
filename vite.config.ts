import fs from "node:fs";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;
const pkg = JSON.parse(
  fs.readFileSync(new URL("./package.json", import.meta.url), "utf8")
) as { version: string };

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // 显式绑定 127.0.0.1。`host: false` 等价于 `localhost`，而 Node 在 Windows 上
    // 会按解析结果只监听一个地址族——实测只绑到 [::1]:1420，`127.0.0.1:1420`
    // 直接不可达，任何按 IPv4 探测 dev server 的工具（脚本、健康检查、部分 WebView
    // 回退路径）都会失败。真机/局域网调试仍通过 TAURI_DEV_HOST 覆盖。
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("node_modules")) {
            if (id.includes("react") || id.includes("scheduler")) {
              return "react-vendor";
            }
            if (id.includes("lucide")) {
              return "lucide-vendor";
            }
            return "vendor";
          }
        },
      },
    },
  },
  test: {
    // 显式限定测试范围：不依赖默认 glob（默认会扫描整个工作目录，
    // 曾导致未跟踪的同名副本 qwitch/src 被一并执行，测试数虚高约一倍）
    include: ["src/**/*.test.{ts,tsx}"],
    exclude: ["**/node_modules/**", "**/dist/**", "**/src-tauri/**"],
    // 组件测试通过文件级 `// @vitest-environment jsdom` 声明环境，
    // 默认保持 node，避免纯函数测试承担 jsdom 启动开销
    environment: "node",
    restoreMocks: true,
  },
}));
