import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import OnboardingWizard from "./components/OnboardingWizard";
import ProjectList from "./components/ProjectList";
import WorkspaceView from "./components/WorkspaceView";
import { isAuthenticationFailure } from "./auth";
import AppShellAccountLink from "./features/account/AppShellAccountLink";

interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
  workspaceId: string;
  tmuxSession: string;
}

function errorSummary(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "primary" in error) {
    return String(error.primary);
  }
  return "Unknown error";
}

export default function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [showWizard, setShowWizard] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<unknown>(null);
  const [startFailures, setStartFailures] = useState<Record<string, unknown>>(
    {},
  );
  const [credentialRequired, setCredentialRequired] = useState<
    Record<string, boolean>
  >({});

  useEffect(() => {
    void loadProjects();
  }, []);

  async function startProject(projectId: string): Promise<unknown | null> {
    setStartFailures((current) => {
      const next = { ...current };
      delete next[projectId];
      return next;
    });

    try {
      setCredentialRequired((current) => ({
        ...current,
        [projectId]: false,
      }));
      await invoke("start_sync", { projectId });
      return null;
    } catch (error) {
      if (isAuthenticationFailure(error)) {
        setCredentialRequired((current) => ({ ...current, [projectId]: true }));
      }
      setStartFailures((current) => ({ ...current, [projectId]: error }));
      return error;
    }
  }

  async function loadProjects() {
    setLoadError(null);
    try {
      const list = await invoke<Project[]>("list_projects");
      setProjects(list);
      setSelectedProject((current) =>
        current
          ? list.find((project) => project.id === current.id) ?? null
          : null,
      );
      setLoading(false);

      list.forEach((project) => {
        void startProject(project.id);
      });
    } catch (error) {
      setLoadError(error);
      setLoading(false);
    }
  }

  if (loading) {
    return (
      <div className="app app-state">
        <AppShellAccountLink />
        <p>Loading projects...</p>
      </div>
    );
  }

  if (loadError) {
    return (
      <div className="app app-state">
        <AppShellAccountLink />
        <div className="load-error" role="alert">
          <strong>Projects could not be loaded</strong>
          <span>{errorSummary(loadError)}</span>
          <button className="btn btn-primary" onClick={() => void loadProjects()}>
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (showWizard || projects.length === 0) {
    return (
      <div className="app">
        <AppShellAccountLink />
        <OnboardingWizard
          onComplete={() => {
            setShowWizard(false);
            void loadProjects();
          }}
          onCancel={() => setShowWizard(false)}
        />
      </div>
    );
  }

  const failedProjects = projects.filter(
    (project) =>
      startFailures[project.id] !== undefined &&
      !isAuthenticationFailure(startFailures[project.id]),
  );

  return (
    <div className="app app-workspace">
      <AppShellAccountLink />
      <aside className="sidebar">
        <div className="sidebar-heading">
          <span>Projects</span>
          <span className="project-count">{projects.length}</span>
        </div>
        <ProjectList
          projects={projects}
          onSelect={(project) =>
            setSelectedProject(
              projects.find((candidate) => candidate.id === project.id) ?? null,
            )
          }
        />
        {failedProjects.length > 0 && (
          <div className="sidebar-alert" role="alert">
            <strong>Sync start failed</strong>
            {failedProjects.map((project) => (
              <span key={project.id}>
                {project.name}: {errorSummary(startFailures[project.id])}
              </span>
            ))}
          </div>
        )}
        {selectedProject && (
          <button
            className="btn btn-sidebar"
            onClick={() => setSelectedProject(null)}
          >
            Back to overview
          </button>
        )}
        <button
          className="btn btn-primary sidebar-new-project"
          onClick={() => setShowWizard(true)}
        >
          New project
        </button>
      </aside>

      {selectedProject ? (
        <WorkspaceView
          project={selectedProject}
          startupFailure={startFailures[selectedProject.id]}
          credentialRequired={credentialRequired[selectedProject.id] === true}
          onRetryStart={() => startProject(selectedProject.id)}
        />
      ) : (
        <main className="main-content empty-workspace">
          <div>
            <strong>Select a project</strong>
            <p>Open its terminal, files, and live sync status.</p>
          </div>
        </main>
      )}
    </div>
  );
}
