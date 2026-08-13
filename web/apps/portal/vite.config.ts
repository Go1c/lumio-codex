import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// 别名必须是绝对路径；用 URL 解析可以避免为此引入 @types/node。
const abs = (relative: string) => new URL(relative, import.meta.url).pathname;

export default defineConfig({
  plugins: [react()],
  resolve: {
    // 顺序有意义：更长的子路径必须排在包名前面。
    alias: {
      "@lumio/ui/styles.css": abs("../../packages/ui/src/styles/index.css"),
      "@lumio/ui/config": abs("../../packages/ui/src/config.ts"),
      "@lumio/ui": abs("../../packages/ui/src/index.ts"),
      "@lumio/auth": abs("../../packages/auth/src/index.ts"),
      "@": abs("./src"),
    },
  },
  server: {
    port: 5280,
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    restoreMocks: true,
  },
});
