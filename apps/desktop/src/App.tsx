import { useCallback, useEffect, useState } from "react";
import { t } from "./i18n";
import { toApiError } from "./lib/api";
import { useApi } from "./state/ApiProvider";
import { useToast } from "./state/ToastProvider";
import { LoginPage } from "./components/LoginPage";
import { ProjectWizard } from "./components/ProjectWizard";
import { Sidebar } from "./components/Sidebar";
import { Workspace } from "./components/Workspace";
import { Modal, Spinner } from "./components/ui";
import { isExpiringSoon } from "./components/AccountMenu";
import type {
  AppInfo,
  ExternalLinks,
  ProjectConfig,
  SessionView,
  SyncStatus,
} from "./lib/types";

/** Heartbeat cadence: refreshes entitlement and reports the device (5.6). */
const HEARTBEAT_MS = 5 * 60_000;

const FALLBACK_LINKS: ExternalLinks = {
  account: "https://cchaven.cn/account",
  invite: "https://cchaven.cn/account#invite",
  docs: "https://cchaven.cn/docs",
  support: "https://cchaven.cn/support",
  serverGuide: "https://cchaven.cn/docs/buy-a-server",
  troubleshooting: "https://cchaven.cn/docs/connection-troubleshooting",
};

type Phase = "restoring" | "signedOut" | "signedIn";

function errorSummary(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "primary" in error) {
    return String(error.primary);
  }
  return "Unknown error";
}

export default function App() {
  const api = useApi();
  const { toast } = useToast();

  const [phase, setPhase] = useState<Phase>("restoring");
  const [session, setSession] = useState<SessionView | null>(null);
  const [signedOutMessage, setSignedOutMessage] = useState<string | null>(null);
  const [offline, setOffline] = useState(false);
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);

  const [projects, setProjects] = useState<ProjectConfig[]>([]);
  const [statuses, setStatuses] = useState<Record<string, SyncStatus>>({});
  const [activeProjectId, setActiveProjectId] = useState<string | null>(null);
  const [activity, setActivity] = useState<string[]>([]);

  const [wizard, setWizard] = useState<{ open: boolean; project: ProjectConfig | null }>({
    open: false,
    project: null,
  });
  const [pendingDelete, setPendingDelete] = useState<ProjectConfig | null>(null);
  const [expiryDismissed, setExpiryDismissed] = useState(false);

  const links = appInfo?.links ?? FALLBACK_LINKS;
  /** 心跳下发的 `expiring_soon` 提醒，与本地 entitlement 判定二选一即可触发横幅。 */
  const [expiryNotice, setExpiryNotice] = useState(false);

  const refreshProjects = useCallback(async () => {
    const list = await api.listProjects();
    setProjects(list);
    const entries = await Promise.all(
      list.map(async (project) => {
        try {
          return [project.id, await api.syncStatus(project.id)] as const;
        } catch {
          return [
            project.id,
            { state: "offline", conflicts: 0, pending: 0 } as SyncStatus,
          ] as const;
        }
      }),
    );
    setStatuses(Object.fromEntries(entries));
    return list;
  }, [api]);

  useEffect(() => {
    void api
      .appInfo()
      .then(setAppInfo)
      .catch(() => setAppInfo(null));
  }, [api]);

  // Silent sign-in at startup (3.4).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const outcome = await api.restoreSession();
        if (cancelled) return;
        if (outcome.state === "signedIn") {
          setSession(outcome.session);
          setPhase("signedIn");
          await refreshProjects();
        } else if (outcome.state === "offline") {
          const cached = await api.listProjects();
          setProjects(cached);
          setOffline(true);
          // 只有存在本地缓存项目时，离线只读才有意义（5.1）。
          setPhase(cached.length > 0 ? "signedIn" : "signedOut");
          setSignedOutMessage(outcome.message);
        } else {
          setSignedOutMessage(outcome.message ?? null);
          setPhase("signedOut");
          setProjects(await api.listProjects());
        }
      } catch (caught) {
        if (cancelled) return;
        setSignedOutMessage(toApiError(caught).message);
        setPhase("signedOut");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [api, refreshProjects]);

  // Heartbeat: entitlement + notices (5.6).
  useEffect(() => {
    if (phase !== "signedIn" || offline) return undefined;
    let cancelled = false;

    const beat = async () => {
      try {
        const result = await api.heartbeat();
        if (cancelled) return;
        setSession((current) =>
          current ? { ...current, entitlement: result.entitlement } : current,
        );
        setOffline(false);
        setExpiryNotice(result.notices.some((notice) => notice.type === "expiring_soon"));
      } catch (caught) {
        if (cancelled) return;
        const failure = toApiError(caught);
        if (failure.code === "network") setOffline(true);
        else if (failure.code === "session_expired" || failure.code === "invalid_grant") {
          setSession(null);
          setSignedOutMessage(t("fixed.sessionExpired"));
          setPhase("signedOut");
        }
      }
    };

    void beat();
    const timer = setInterval(() => void beat(), HEARTBEAT_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [api, phase, offline]);

  const activeProject = projects.find((project) => project.id === activeProjectId) ?? null;

  async function onSignedIn(next: SessionView) {
    setSession(next);
    setSignedOutMessage(null);
    setOffline(false);
    setPhase("signedIn");
    await refreshProjects();
  }

  async function logout() {
    await api.logout();
    setSession(null);
    setActiveProjectId(null);
    setPhase("signedOut");
    setSignedOutMessage(null);
  }

  async function confirmDelete(project: ProjectConfig) {
    await api.deleteProject(project.id);
    setPendingDelete(null);
    if (activeProjectId === project.id) setActiveProjectId(null);
    await refreshProjects();
    toast(t("deleteProject.done", { name: project.name }));
  }

  if (phase === "restoring") {
    return (
      <div className="auth-page">
        <div className="auth-card">
          <div className="logo">
            <span className="mark" aria-hidden="true" />
            {t("brand.name")}
          </div>
          <p className="sub">
            <Spinner dark />
            {t("common.loading")}
          </p>
        </div>
      </div>
    );
  }

  if (phase === "signedOut") {
    return (
      <LoginPage
        onSignedIn={(next) => void onSignedIn(next)}
        onUseOffline={() => {
          setOffline(true);
          setPhase("signedIn");
        }}
        canUseOffline={projects.length > 0}
        initialMessage={signedOutMessage}
      />
    );
  }

  const showExpiryBanner =
    !expiryDismissed && !offline && (expiryNotice || isExpiringSoon(session?.entitlement));

  return (
    <div className="appframe">
      <Sidebar
        projects={projects}
        statuses={statuses}
        activeProjectId={activeProjectId}
        session={session}
        links={links}
        offline={offline}
        activity={activity}
        onSelectProject={setActiveProjectId}
        onNewProject={() => setWizard({ open: true, project: null })}
        onEditProject={(project) => setWizard({ open: true, project })}
        onRevealProject={(project) => void api.revealEntry(project.id)}
        onDeleteProject={setPendingDelete}
        onOpenExternal={(url) => void api.openExternal(url)}
        onLogout={() => void logout()}
        onSyncBarClick={() => {
          const withConflicts = projects.find((project) => statuses[project.id]?.conflicts);
          if (withConflicts) setActiveProjectId(withConflicts.id);
        }}
      />

      <main className="app-main">
        {offline && (
          <div className="top-banner offline">
            <span>{t("offline.banner")}</span>
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => {
                void api.restoreSession().then((outcome) => {
                  if (outcome.state === "signedIn") {
                    setOffline(false);
                    setSession(outcome.session);
                    void refreshProjects();
                  }
                });
              }}
            >
              {t("offline.retry")}
            </button>
          </div>
        )}

        {showExpiryBanner && (
          <div className="top-banner">
            <span>{t("account.expiringBanner")}</span>
            <span style={{ display: "flex", gap: 8 }}>
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void api.openExternal(links.account)}
              >
                {t("account.manage")}
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={() => setExpiryDismissed(true)}
              >
                {t("common.close")}
              </button>
            </span>
          </div>
        )}

        {activeProject ? (
          <Workspace
            project={activeProject}
            status={statuses[activeProject.id] ?? { state: "synced", conflicts: 0, pending: 0 }}
            offline={offline}
            onStatusChanged={async () => {
              await refreshProjects();
              setActivity((current) => [`${activeProject.name}：冲突已更新`, ...current].slice(0, 8));
            }}
          />
        ) : (
          <div className="empty-state">
            <div className="art" aria-hidden="true">
              🖥️ ⇄ ☁️
            </div>
            <h3>{t("emptyState.title")}</h3>
            <p>{t("emptyState.body")}</p>
            <button
              type="button"
              className="btn btn-primary btn-lg"
              onClick={() => setWizard({ open: true, project: null })}
            >
              {t("emptyState.action")}
            </button>
          </div>
        )}
      </main>

      {wizard.open && (
        <ProjectWizard
          project={wizard.project}
          // 取消后回到空状态页（修复现状 bug）：只关闭模态，不改变主区。
          onCancel={() => setWizard({ open: false, project: null })}
          onCompleted={(project) => {
            setWizard({ open: false, project: null });
            void refreshProjects().then(() => setActiveProjectId(project.id));
          }}
        />
      )}

      {pendingDelete && (
        <Modal
          small
          title={t("deleteProject.title", { name: pendingDelete.name })}
          onClose={() => setPendingDelete(null)}
        >
          <h2 style={{ fontSize: 17 }}>{t("deleteProject.title", { name: pendingDelete.name })}</h2>
          <p style={{ color: "var(--gray)", fontSize: 13.5, margin: "12px 0 22px", lineHeight: 1.7 }}>
            {t("deleteProject.body")}
          </p>
          <div className="wizard-nav" style={{ marginTop: 0 }}>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => setPendingDelete(null)}
            >
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="btn btn-danger"
              onClick={() => void confirmDelete(pendingDelete)}
            >
              {t("deleteProject.confirm")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
