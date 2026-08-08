import { useState } from "react";
import TerminalPane from "./Terminal";

interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
  tmuxSession: string;
}

export default function WorkspaceView({ project }: { project: Project }) {
  const [activeTab, setActiveTab] = useState<"terminal" | "files">("terminal");

  return (
    <div className="main-content">
      <div
        style={{
          display: "flex",
          borderBottom: "1px solid #d0d0d0",
          background: "#fff",
        }}
      >
        <button
          className={`btn ${activeTab === "terminal" ? "btn-primary" : "btn-secondary"}`}
          style={{ borderRadius: 0, marginRight: "4px" }}
          onClick={() => setActiveTab("terminal")}
        >
          Terminal
        </button>
        <button
          className={`btn ${activeTab === "files" ? "btn-primary" : "btn-secondary"}`}
          style={{ borderRadius: 0 }}
          onClick={() => setActiveTab("files")}
        >
          Files
        </button>
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
        {activeTab === "files" && (
          <div style={{ padding: "40px", color: "#888" }}>
            File browser will be available in a future update.
          </div>
        )}
      </div>
    </div>
  );
}
