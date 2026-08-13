import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App";
import "./styles.css";

// 开发期默认走 MSW mock（本机无法启动控制面的 PostgreSQL）。
// 要连真实后端：VITE_USE_MOCK=false npm run dev，请求经 vite 代理转给 /api。
// 生产构建里 import.meta.env.DEV 为 false，这段连同 mock 代码一起被摇掉。
async function bootstrap() {
  if (import.meta.env.DEV && import.meta.env.VITE_USE_MOCK !== "false") {
    const { startMockWorker } = await import("./mocks/browser");
    await startMockWorker();
  }

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </StrictMode>,
  );
}

void bootstrap();
