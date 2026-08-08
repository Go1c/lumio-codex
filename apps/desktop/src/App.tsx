import { useState, useEffect } from "react";
import OnboardingWizard from "./components/OnboardingWizard";
import ProjectList from "./components/ProjectList";
import { invoke } from "@tauri-apps/api/core";

interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
}

export default function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [showWizard, setShowWizard] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    loadProjects();
  }, []);

  async function loadProjects() {
    try {
      const list = await invoke<Project[]>("list_projects");
      setProjects(list);
    } catch (e) {
      console.error("Failed to load projects:", e);
    } finally {
      setLoading(false);
    }
  }

  if (loading) {
    return <div className="app">Loading...</div>;
  }

  if (showWizard || projects.length === 0) {
    return (
      <div className="app">
        <OnboardingWizard
          onComplete={() => {
            setShowWizard(false);
            loadProjects();
          }}
          onCancel={() => setShowWizard(false)}
        />
      </div>
    );
  }

  return (
    <div className="app" style={{ flexDirection: "row" }}>
      <div className="sidebar">
        <h2 style={{ fontSize: "16px", marginBottom: "16px" }}>Projects</h2>
        <ProjectList projects={projects} />
        <button
          className="btn btn-primary"
          style={{ marginTop: "auto" }}
          onClick={() => setShowWizard(true)}
        >
          + New Project
        </button>
      </div>
      <div className="main-content">
        <p style={{ padding: "40px", color: "#888" }}>
          Select a project or create a new one to get started.
        </p>
      </div>
    </div>
  );
}
