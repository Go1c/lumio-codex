import { useState } from "react";
import TerminalPane from "./Terminal";
import FileTree from "./FileTree";

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

  const tabs: { key: Tab; label: string }[] = [
    { key: "terminal", label: "Terminal" },
    { key: "files", label: "Files" },
    { key: "conflicts", label: "Conflicts" },
  ];

  return (
    <div className="main-content">
      <div
        style={{
          display: "flex",
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
