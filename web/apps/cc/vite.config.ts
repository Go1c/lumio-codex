import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const abs = (relative: string) => new URL(relative, import.meta.url).pathname;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@lumio/ui/styles.css": abs("../../packages/ui/src/styles/index.css"),
      "@lumio/ui/config": abs("../../packages/ui/src/config.ts"),
      "@lumio/ui": abs("../../packages/ui/src/index.ts"),
      "@lumio/auth": abs("../../packages/auth/src/index.ts"),
      "@": abs("./src"),
    },
  },
  server: {
    port: 5281,
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    restoreMocks: true,
  },
});
