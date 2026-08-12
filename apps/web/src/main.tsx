import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";

import { App } from "./App";
import "./styles.css";

async function bootstrap() {
  // 本机开发默认走 MSW：M1 控制面依赖 PostgreSQL，本机起不来（见 README「Mock 说明」）。
  if (import.meta.env.DEV && import.meta.env.VITE_ENABLE_MSW !== "false") {
    const { worker } = await import("./mocks/browser");
    await worker.start({ onUnhandledRequest: "bypass" });
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
