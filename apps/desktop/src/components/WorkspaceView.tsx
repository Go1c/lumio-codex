import { useState, useEffect } from "react";
import TerminalPane from "./Terminal";
import FileTree from "./FileTree";
import { invoke } from "@tauri-apps/api/core";

interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
  tmuxSession: string;
}

type Tab = "terminal" | "files" | "conflicts";

export default function WorkspaceView({ project }: { project: Project }) {
  const [activeTab, setActiveTab] = useState<Tab>("terminal");
  const [syncState, setSyncState] = useState<string>("starting");

  // Auto-start sync when the project opens.
  useEffect(() => {
    let cancelled = false;
    setSyncState("starting");

    invoke("start_sync", { projectId: project.id })
      .then(() => {
        if (!cancelled) setSyncState("synced");
      })
      .catch((e) => {
        console.error("Sync start failed:", e);
        if (!cancelled) setSyncState("error");
      });

    // Stop sync when leaving the project.
    return () => {
      cancelled = true;
      invoke("stop_sync", { projectId: project.id }).catch(() => {});
    };
  }, [project.id]);

  const tabs: { key: Tab; label: string }[] = [
    { key: "terminal", label: "Terminal" },
    { key: "files", label: "Files" },
    { key: "conflicts", label: "Conflicts" },
  ];

  const syncIndicator = () => {
    const styles: Record<string, { color: string; bg: string; text: string }> = {
      starting: { color: "#f59e0b", bg: "#fef3c7", text: "⟳ Syncing..." },
      synced: { color: "#16a34a", bg: "#dcfce7", text: "● Synced" },
      error: { color: "#dc2626", bg: "#fee2e2", text: "✕ Sync Error" },
    };
    const s = styles[syncState] || styles.starting;
    return (
      <span
        style={{
          fontSize: "11px",
          color: s.color,
          background: s.bg,
          padding: "2px 8px",
          borderRadius: "4px",
          marginLeft: "8px",
        }}
      >
        {s.text}
      </span>
    );
  };

  return (
    <div className="main-content">
      <div
        style={{
          display: "flex",
          alignItems: "center",
          borderBottom: "1px solid #d0d0d0",
          background: "#fff",
        }}
      >
        {tabs.map((tab) => (
          <button
            key={tab.key}
            className={`btn ${activeTab === tab.key ? "btn-primary" : "btn-secondary"}`}
            style={{ borderRadius: 0, marginRight: "4px" }}
            onClick={() => setActiveTab(tab.key)}
          >
            {tab.label}
          </button>
        ))}
        <div style={{ marginLeft: "auto", paddingRight: "12px" }}>
          {syncIndicator()}
        </div>
      </div>
      <div style={{ flex: 1, overflow: "hidden" }}>
        {activeTab === "terminal" && (
          <TerminalPane
            projectId={project.id}
            sshHostAlias={project.sshHostAlias}
            remoteRoot={project.remoteRoot}
            tmuxSession={project.tmuxSession || `fns-${project.name}`}
          />
        )}
        {activeTab === "files" && <FileTree localRoot={project.localRoot} />}
        {activeTab === "conflicts" && (
          <div style={{ padding: "40px", color: "#888" }}>
            No sync conflicts.
          </div>
        )}
      </div>
    </div>
  );
}
