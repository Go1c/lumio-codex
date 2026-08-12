import { useEffect, useMemo, useRef, useState, type ClipboardEvent } from "react";
import { t, tList } from "../i18n";
import { EVENTS, toApiError } from "../lib/api";
import { useApi } from "../state/ApiProvider";
import { useToast } from "../state/ToastProvider";
import { Banner, Modal, Spinner } from "./ui";
import {
  DEPLOY_STAGES,
  type AuthMethod,
  type DeployStage,
  type ProjectConfig,
  type ProbeResult,
  type SshHost,
  type StageState,
  type StageUpdate,
} from "../lib/types";

const DEFAULT_EXCLUDES = [".git/", "node_modules/", "target/", ".env"].join("\n");

type TestState = "idle" | "testing" | "ok" | "fail";

export interface ProjectWizardProps {
  /** Pre-filled project for 「编辑」; absent when creating. */
  project?: ProjectConfig | null;
  onCancel: () => void;
  onCompleted: (project: ProjectConfig) => void;
}

/** 5.3 新建／编辑项目向导 — three steps, modal, defaults over decisions. */
export function ProjectWizard({ project, onCancel, onCompleted }: ProjectWizardProps) {
  const api = useApi();
  const { toast } = useToast();
  const editing = Boolean(project);

  const [step, setStep] = useState(0);
  const [error, setError] = useState("");

  // Step 1 — connect to the server.
  const [host, setHost] = useState(project?.server.host ?? "");
  const [user, setUser] = useState(project?.server.user ?? "root");
  const [password, setPassword] = useState("");
  const [port, setPort] = useState(String(project?.server.port ?? 22));
  const [auth, setAuth] = useState<AuthMethod>(project?.server.auth ?? "password");
  const [keyPath, setKeyPath] = useState(project?.server.keyPath ?? "");
  const [configAlias, setConfigAlias] = useState(project?.server.configAlias ?? "");
  const [sshHosts, setSshHosts] = useState<SshHost[]>([]);
  const [test, setTest] = useState<TestState>(editing ? "ok" : "idle");
  const [probe, setProbe] = useState<ProbeResult | null>(null);

  // Step 2 — project settings.
  const [name, setName] = useState(project?.name ?? "");
  const [remoteEdited, setRemoteEdited] = useState(false);
  const [remoteRoot, setRemoteRoot] = useState(project?.remoteRoot ?? "");
  const [localEdited, setLocalEdited] = useState(false);
  const [localRoot, setLocalRoot] = useState(project?.localRoot ?? "");
  const [presetRemote, setPresetRemote] = useState(project?.remoteRoot ?? "");
  const [presetLocal, setPresetLocal] = useState(project?.localRoot ?? "");
  const [tmuxSession, setTmuxSession] = useState(project?.tmuxSession ?? "");
  const [excludes, setExcludes] = useState(
    project?.sync.excludes.join("\n") ?? DEFAULT_EXCLUDES,
  );

  // Step 3 — deployment.
  const [deploying, setDeploying] = useState(false);
  const [stages, setStages] = useState<Record<DeployStage, StageState>>({
    connect: "pending",
    installAgent: "pending",
    createDirectory: "pending",
    firstSync: "pending",
  });
  const [details, setDetails] = useState<Partial<Record<DeployStage, string>>>({});
  const [deployError, setDeployError] = useState("");
  const [links, setLinks] = useState<{ serverGuide: string; troubleshooting: string } | null>(
    null,
  );
  const projectIdRef = useRef(project?.id ?? crypto.randomUUID());

  useEffect(() => {
    void api.sshHosts().then(setSshHosts).catch(() => setSshHosts([]));
    void api
      .appInfo()
      .then((info) => setLinks(info.links))
      .catch(() => setLinks(null));
  }, [api]);

  // Directory presets follow the name and the login user until the user edits
  // them by hand (5.3 第 2 步).
  useEffect(() => {
    let cancelled = false;
    void api.projectPresets(name || "my-project", user).then((presets) => {
      if (cancelled) return;
      setPresetRemote(presets.remoteRoot);
      setPresetLocal(presets.localRoot);
      setTmuxSession(presets.tmuxSession);
    });
    return () => {
      cancelled = true;
    };
  }, [api, name, user]);

  useEffect(() => {
    const dispose = api.on<StageUpdate>(EVENTS.deployProgress, (update) => {
      if (update.projectId !== projectIdRef.current) return;
      setStages((current) => ({ ...current, [update.stage]: update.state }));
      if (update.detail) {
        setDetails((current) => ({ ...current, [update.stage]: update.detail as string }));
      }
    });
    return () => {
      void dispose.then((unsubscribe) => unsubscribe());
    };
  }, [api]);

  const effectiveRemote = remoteEdited ? remoteRoot : presetRemote;
  const effectiveLocal = localEdited ? localRoot : presetLocal;

  const stageLabels = useMemo<Record<DeployStage, string>>(
    () => ({
      connect: t("wizard.stageConnect"),
      installAgent: t("wizard.stageInstall"),
      createDirectory: t("wizard.stageDirectory", { path: effectiveRemote }),
      firstSync: details.firstSync
        ? t("wizard.stageSyncProgress", { detail: details.firstSync })
        : t("wizard.stageSync"),
    }),
    [effectiveRemote, details.firstSync],
  );

  async function handlePaste(event: ClipboardEvent<HTMLInputElement>) {
    const text = event.clipboardData.getData("text");
    const parsed = await api.parsePastedTarget(text);
    if (!parsed || parsed.host === text.trim()) return;
    event.preventDefault();
    setHost(parsed.host);
    if (parsed.user) setUser(parsed.user);
    if (parsed.port) setPort(String(parsed.port));
    setTest("idle");
    toast(parsed.user ? t("wizard.pasteDetected") : t("wizard.pasteDetectedHostOnly"));
  }

  function serverConfig() {
    return {
      host: host.trim(),
      user: user.trim() || "root",
      port: Number(port) || 22,
      auth,
      keyPath: auth === "key" ? keyPath || null : null,
      configAlias: auth === "ssh_config" ? configAlias || null : null,
    };
  }

  async function connectAndContinue() {
    setError("");
    if (auth !== "ssh_config" && !host.trim()) {
      setError(t("wizard.needAddress"));
      return;
    }
    if (auth === "password" && !password && !editing) {
      setError(t("wizard.needPassword"));
      return;
    }

    setTest("testing");
    const result = await api.testConnection(serverConfig(), password || undefined);
    setProbe(result);
    if (!result.ok) {
      setTest("fail");
      return;
    }
    setTest("ok");
    setStep(1);
  }

  function goToSummary() {
    setError("");
    if (!name.trim()) {
      setError(t("wizard.needName"));
      return;
    }
    if (remoteEdited && !remoteRoot.startsWith("/")) {
      setError(t("wizard.remoteMustBeAbsolute"));
      return;
    }
    setStep(2);
  }

  function buildConfig(): ProjectConfig {
    return {
      id: projectIdRef.current,
      name: name.trim(),
      server: serverConfig(),
      remoteRoot: effectiveRemote,
      localRoot: effectiveLocal,
      workspaceId: project?.workspaceId ?? crypto.randomUUID(),
      tmuxSession: tmuxSession || `cchaven-${name.trim()}`,
      sync: {
        mode: "two_way_safe",
        includes: ["**"],
        excludes: excludes
          .split("\n")
          .map((line) => line.trim())
          .filter(Boolean),
        // 硬性要求：机密文件永不同步，M3 不提供关闭入口。
        protectSecrets: true,
      },
      createdAt: project?.createdAt ?? "",
    };
  }

  async function save() {
    setError("");
    try {
      const saved = await api.saveProject(buildConfig(), password || undefined);
      toast(t("wizard.saved"));
      onCompleted(saved);
    } catch (caught) {
      setError(toApiError(caught).message);
    }
  }

  async function deploy(fromStage = 0) {
    setDeploying(true);
    setDeployError("");
    setStages((current) => {
      const next = { ...current };
      DEPLOY_STAGES.slice(fromStage).forEach((stage) => {
        next[stage] = "pending";
      });
      return next;
    });

    try {
      const saved = await api.saveProject(buildConfig(), password || undefined);
      await api.deployProject(saved.id, fromStage);
      onCompleted(saved);
    } catch (caught) {
      const failure = toApiError(caught);
      setDeployError(failure.message);
      if (failure.stage) {
        setStages((current) => ({ ...current, [failure.stage as DeployStage]: "failed" }));
      }
    } finally {
      setDeploying(false);
    }
  }

  const failedStage = DEPLOY_STAGES.find((stage) => stages[stage] === "failed");
  const started = DEPLOY_STAGES.some((stage) => stages[stage] !== "pending");

  return (
    <Modal
      title={editing ? t("wizard.editTitle") : t("wizard.createTitle")}
      onClose={onCancel}
      dismissible={!deploying}
    >
      <h2>{editing ? t("wizard.editTitle") : t("wizard.createTitle")}</h2>
      <div className="wizard-steps">
        {tList("wizard.steps").map((label, index) => (
          <div key={label} className={index <= step ? "on" : ""}>
            {index + 1}. {label}
          </div>
        ))}
      </div>

      {step === 0 && (
        <>
          <div className="helper-box">
            💡 {t("wizard.helper")}
            <br />
            <a
              href={links?.serverGuide ?? "#"}
              onClick={(event) => {
                event.preventDefault();
                if (links) void api.openExternal(links.serverGuide);
              }}
            >
              {t("wizard.helperLink")}
            </a>
          </div>

          <div className="field">
            <label htmlFor="wizard-host">{t("wizard.addressLabel")}</label>
            <input
              id="wizard-host"
              value={host}
              placeholder={t("wizard.addressPlaceholder")}
              onChange={(event) => {
                setHost(event.target.value);
                setTest("idle");
              }}
              onPaste={handlePaste}
              disabled={test === "testing"}
            />
            <div className="hint">{t("wizard.addressHint")}</div>
          </div>

          <div className="field-row">
            <div className="field">
              <label htmlFor="wizard-user">{t("wizard.userLabel")}</label>
              <input
                id="wizard-user"
                value={user}
                onChange={(event) => {
                  setUser(event.target.value);
                  setTest("idle");
                }}
                disabled={test === "testing"}
              />
              <div className="hint">{t("wizard.userHint")}</div>
            </div>
            {auth === "password" && (
              <div className="field">
                <label htmlFor="wizard-password">{t("wizard.passwordLabel")}</label>
                <input
                  id="wizard-password"
                  type="password"
                  value={password}
                  placeholder={t("wizard.passwordPlaceholder")}
                  onChange={(event) => {
                    setPassword(event.target.value);
                    setTest("idle");
                  }}
                  disabled={test === "testing"}
                />
                <div className="hint">{t("wizard.passwordHint")}</div>
              </div>
            )}
          </div>

          <details className="advanced">
            <summary>{t("wizard.advanced")}</summary>
            <div className="field-row" style={{ marginTop: 12 }}>
              <div className="field">
                <label htmlFor="wizard-port">{t("wizard.portLabel")}</label>
                <input
                  id="wizard-port"
                  value={port}
                  onChange={(event) => setPort(event.target.value)}
                />
              </div>
              <div className="field">
                <label htmlFor="wizard-auth">{t("wizard.authLabel")}</label>
                <select
                  id="wizard-auth"
                  value={auth}
                  onChange={(event) => {
                    setAuth(event.target.value as AuthMethod);
                    setTest("idle");
                  }}
                >
                  <option value="password">{t("wizard.authPassword")}</option>
                  <option value="key">{t("wizard.authKey")}</option>
                  <option value="ssh_config">{t("wizard.authSshConfig")}</option>
                </select>
              </div>
            </div>
            {auth === "key" && (
              <div className="field">
                <label htmlFor="wizard-key">{t("wizard.keyPathLabel")}</label>
                <input
                  id="wizard-key"
                  value={keyPath}
                  placeholder="~/.ssh/id_ed25519"
                  onChange={(event) => setKeyPath(event.target.value)}
                />
              </div>
            )}
            {auth === "ssh_config" && (
              <div className="field">
                <label htmlFor="wizard-alias">{t("wizard.sshConfigLabel")}</label>
                <select
                  id="wizard-alias"
                  value={configAlias}
                  onChange={(event) => setConfigAlias(event.target.value)}
                >
                  <option value="">—</option>
                  {sshHosts.map((entry) => (
                    <option key={entry.alias} value={entry.alias}>
                      {entry.alias}
                      {entry.hostname ? `（${entry.hostname}）` : ""}
                    </option>
                  ))}
                </select>
              </div>
            )}
          </details>

          {test === "ok" && (
            <Banner tone="ok">
              {t("wizard.connectOk", { distro: probe?.distro ?? "Linux" })}
            </Banner>
          )}
          {test === "fail" && (
            <Banner tone="error" block>
              <strong>{t("wizard.connectFailTitle")}</strong>
              <ol className="troubleshooting">
                {orderedChecks(probe, Number(port) || 22).map((check) => (
                  <li key={check}>{check}</li>
                ))}
              </ol>
              <a
                href={links?.troubleshooting ?? "#"}
                onClick={(event) => {
                  event.preventDefault();
                  if (links) void api.openExternal(links.troubleshooting);
                }}
              >
                {t("wizard.connectFailLink")}
              </a>
            </Banner>
          )}
        </>
      )}

      {step === 1 && (
        <>
          <div className="field">
            <label htmlFor="wizard-name">{t("wizard.nameLabel")}</label>
            <input
              id="wizard-name"
              value={name}
              placeholder={t("wizard.namePlaceholder")}
              onChange={(event) => setName(event.target.value)}
            />
            <div className="hint">{t("wizard.nameHint")}</div>
          </div>

          <PresetField
            id="wizard-remote"
            label={t("wizard.remoteLabel")}
            hint={t("wizard.remoteHint")}
            preset={presetRemote}
            edited={remoteEdited}
            value={remoteRoot}
            onEdit={() => {
              setRemoteEdited(true);
              setRemoteRoot(presetRemote);
            }}
            onChange={setRemoteRoot}
            onReset={() => setRemoteEdited(false)}
          />

          <PresetField
            id="wizard-local"
            label={t("wizard.localLabel")}
            hint={t("wizard.localHint")}
            preset={presetLocal}
            edited={localEdited}
            value={localRoot}
            onEdit={() => {
              setLocalEdited(true);
              setLocalRoot(presetLocal);
            }}
            onChange={setLocalRoot}
            onReset={() => setLocalEdited(false)}
          />

          <details className="advanced">
            <summary>{t("wizard.excludesSummary")}</summary>
            <div className="field" style={{ marginTop: 12 }}>
              <label htmlFor="wizard-excludes">{t("wizard.excludesLabel")}</label>
              <textarea
                id="wizard-excludes"
                rows={4}
                value={excludes}
                onChange={(event) => setExcludes(event.target.value)}
              />
            </div>
            {/* 机密保护是固定提示，不是开关（硬性要求）。 */}
            <Banner tone="ok">{t("wizard.protectSecrets")}</Banner>
          </details>
        </>
      )}

      {step === 2 && (
        <>
          <table className="summary-table">
            <tbody>
              <tr>
                <td>{t("wizard.summaryProject")}</td>
                <td>
                  <strong>{name}</strong>
                </td>
              </tr>
              <tr>
                <td>{t("wizard.summaryServer")}</td>
                <td>
                  {user}@{host}{" "}
                  <span style={{ color: "var(--green)", fontSize: 12.5 }}>
                    {t("wizard.summaryVerified")}
                  </span>
                </td>
              </tr>
              <tr>
                <td>{t("wizard.summaryRemote")}</td>
                <td className="mono">{effectiveRemote}</td>
              </tr>
              <tr>
                <td>{t("wizard.summaryLocal")}</td>
                <td className="mono">{effectiveLocal}</td>
              </tr>
            </tbody>
          </table>

          {started ? (
            <div style={{ margin: "14px 0 4px" }}>
              {DEPLOY_STAGES.map((stage) => (
                <div
                  key={stage}
                  className={`deploy-stage ${stages[stage] === "pending" ? "pending" : ""}`}
                >
                  <span className="st">
                    {stages[stage] === "running" && <Spinner dark />}
                    {stages[stage] === "done" && (
                      <span style={{ color: "var(--green)" }}>✓</span>
                    )}
                    {stages[stage] === "failed" && <span style={{ color: "var(--red)" }}>✗</span>}
                    {stages[stage] === "pending" && <span style={{ color: "#bbb" }}>○</span>}
                  </span>
                  <span>{stageLabels[stage]}</span>
                </div>
              ))}
              {deployError && <Banner tone="error">{deployError}</Banner>}
            </div>
          ) : (
            <p style={{ color: "var(--gray)", fontSize: 13.5, marginTop: 8 }}>
              {t("wizard.summaryHint")}
            </p>
          )}
        </>
      )}

      {error && <p style={{ color: "var(--red)", fontSize: 13, marginTop: 12 }}>{error}</p>}

      <div className="wizard-nav">
        {step > 0 ? (
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => {
              setError("");
              setStep(step - 1);
            }}
            disabled={deploying}
          >
            {t("common.back")}
          </button>
        ) : (
          <button
            type="button"
            className="btn btn-secondary"
            onClick={onCancel}
            disabled={test === "testing"}
          >
            {t("common.cancel")}
          </button>
        )}

        {step === 0 && (
          <button
            type="button"
            className="btn btn-primary"
            onClick={connectAndContinue}
            disabled={test === "testing"}
          >
            {test === "testing" && <Spinner />}
            {test === "testing" ? t("wizard.connecting") : t("wizard.connectAndContinue")}
          </button>
        )}

        {step === 1 && (
          <button type="button" className="btn btn-primary" onClick={goToSummary}>
            {t("common.next")}
          </button>
        )}

        {step === 2 &&
          (editing ? (
            <button type="button" className="btn btn-primary" onClick={save}>
              {t("wizard.saveChanges")}
            </button>
          ) : failedStage ? (
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => deploy(DEPLOY_STAGES.indexOf(failedStage))}
            >
              {t("common.retry")}
            </button>
          ) : (
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => deploy(0)}
              disabled={deploying}
            >
              {deploying && <Spinner />}
              {deploying ? t("wizard.deploying") : t("wizard.finish")}
            </button>
          ))}
      </div>
    </Modal>
  );
}

/** Troubleshooting list, most-likely cause first (5.3 失败态). */
function orderedChecks(probe: ProbeResult | null, port: number): string[] {
  const address = t("wizard.connectFail1");
  const credentials = t("wizard.connectFail2");
  const firewall = t("wizard.connectFail3", { port });
  if (probe?.failure === "auth") return [credentials, address, firewall];
  return [address, credentials, firewall];
}

function PresetField({
  id,
  label,
  hint,
  preset,
  edited,
  value,
  onEdit,
  onChange,
  onReset,
}: {
  id: string;
  label: string;
  hint: string;
  preset: string;
  edited: boolean;
  value: string;
  onEdit: () => void;
  onChange: (value: string) => void;
  onReset: () => void;
}) {
  return (
    <div className="field">
      {edited ? <label htmlFor={id}>{label}</label> : <span className="field-label">{label}</span>}
      {!edited ? (
        <div className="preset-row">
          {/* aria-label keeps the read-only preset row reachable by its field name. */}
          <code id={id} aria-label={label}>
            {preset}
          </code>
          <span className="tag-auto">{t("wizard.autoSet")}</span>
          <button type="button" className="btn btn-ghost btn-sm" onClick={onEdit}>
            {t("wizard.change")}
          </button>
        </div>
      ) : (
        <div className="preset-edit">
          <input id={id} value={value} onChange={(event) => onChange(event.target.value)} />
          <button type="button" className="btn btn-secondary" onClick={onReset}>
            {t("wizard.useRecommended")}
          </button>
        </div>
      )}
      <div className="hint">{hint}</div>
    </div>
  );
}
