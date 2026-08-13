import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// 不引入 @types/node：用 URL 解析出绝对路径即可，别名必须是绝对路径。
const abs = (relative: string) => new URL(relative, import.meta.url).pathname;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@lumio/ui/config": abs("../ui/src/config.ts"),
      "@lumio/ui": abs("../ui/src/index.ts"),
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    restoreMocks: true,
  },
});
