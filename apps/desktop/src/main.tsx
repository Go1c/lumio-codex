import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { createTauriApi, isTauri, type Api } from "./lib/api";
import { MockApi, sampleConflicts, sampleFiles, sampleProject } from "./lib/mockApi";
import { ApiProvider } from "./state/ApiProvider";
import { ToastProvider } from "./state/ToastProvider";
import "./styles.css";

/**
 * In the Tauri webview we talk to the Rust backend; `npm run dev` in a plain
 * browser falls back to the in-memory mock so the UI can be built without a
 * server or a control plane (see README).
 */
async function resolveApi(): Promise<Api> {
  if (!isTauri()) {
    return new MockApi({
      signedIn: false,
      authDelayMs: 1200,
      invited: true,
      projects: [sampleProject()],
      conflicts: sampleConflicts(),
      files: sampleFiles(),
    });
  }
  const [{ invoke }, { listen }] = await Promise.all([
    import("@tauri-apps/api/core"),
    import("@tauri-apps/api/event"),
  ]);
  return createTauriApi(
    (command, args) => invoke(command, args),
    (event, handler) => listen(event, handler),
  );
}

void resolveApi().then((api) => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ApiProvider api={api}>
        <ToastProvider>
          <App />
        </ToastProvider>
      </ApiProvider>
    </React.StrictMode>,
  );
});
