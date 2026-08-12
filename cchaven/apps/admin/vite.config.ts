import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// 后台是独立入口（部署为 admin.cchaven.cn），与官网、APP 完全隔离。
// 开发期默认走 MSW mock；把 VITE_USE_MOCK 设为 false 即改打真实控制面。
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5183,
    proxy: {
      "/api": {
        target: process.env.VITE_API_ORIGIN ?? "http://localhost:8080",
        changeOrigin: false,
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    restoreMocks: true,
    // 六个 jsdom + MSW 的文件并行跑会互相抢资源，导致等待超时假失败。
    fileParallelism: false,
    testTimeout: 20_000,
  },
});
