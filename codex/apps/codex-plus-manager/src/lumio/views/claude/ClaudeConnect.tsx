import { open } from "@tauri-apps/plugin-dialog";
import { type ClipboardEvent, type FormEvent, useEffect, useState } from "react";

import {
  formatSetupElapsed,
  listClaudeSshHosts,
  prepareErrorCopy,
  probeErrorCopy,
  setupPhaseCopy,
  syncErrorCopy,
} from "../../claude/api.ts";
import { HELP_URL } from "../../help.ts";
import { openInBrowser } from "../../invoke.ts";
import { CONNECT_STEPS } from "../../claude/machine.ts";
import { folderNameFromPath, localProjectRoot, remoteProjectRoot } from "../../claude/paths.ts";
import { cancelClaudeConnect, runConnectProbe, runConnectSetup, runConnectSync } from "../../claude/session.ts";
import { dispatchClaude, draftPassword, setDraftPassword } from "../../claude/store.ts";
import type { ClaudeConnectSheet, ClaudeConnectStep, ClaudeSshHost } from "../../claude/types.ts";

const TROUBLESHOOTING = [
  { title: "IP 是否抄对", detail: "用控制台里的公网 IP，不要填内网地址。" },
  { title: "密码是否正确", detail: "注意大小写，建议从控制台重新复制。" },
  { title: "安全组是否放行 22", detail: "云平台防火墙要允许你当前网络访问 SSH 端口。" },
] as const;

function stepIndex(step: ClaudeConnectStep): number {
  return CONNECT_STEPS.indexOf(step);
}

export function ClaudeConnect({
  sheet,
  onBackToCodex,
}: {
  sheet: ClaudeConnectSheet;
  onBackToCodex: () => void;
}) {
  const [password, setPassword] = useState(() => draftPassword());
  const [sshHosts, setSshHosts] = useState<ClaudeSshHost[]>([]);
  const [setupElapsed, setSetupElapsed] = useState(0);
  const draft = sheet.draft;
  const probing = sheet.probeStatus === "running";
  const installing = sheet.setupStatus === "running";
  const remote = draft.remoteRoot.trim() || remoteProjectRoot(draft.user, draft.projectName);
  const local = draft.localRoot.trim() || localProjectRoot(draft.projectName);
  const foldersReady = local !== "" && remote !== "";
  const current = stepIndex(sheet.step);
  const isNewProject = sheet.mode === "project";

  const onPassword = (value: string) => {
    setPassword(value);
    setDraftPassword(value);
  };

  const onHostPaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const text = event.clipboardData.getData("text");
    if (!text.includes("@") && !text.toLowerCase().includes("ssh")) return;
    event.preventDefault();
    dispatchClaude({ type: "ssh-pasted", text });
  };

  useEffect(() => {
    void listClaudeSshHosts().then(setSshHosts);
  }, []);

  useEffect(() => {
    if (!installing) {
      setSetupElapsed(0);
      return;
    }
    const started = Date.now();
    const tick = window.setInterval(() => {
      setSetupElapsed(Math.floor((Date.now() - started) / 1000));
    }, 1000);
    return () => window.clearInterval(tick);
  }, [installing]);

  const onHostSubmit = (event: FormEvent) => {
    event.preventDefault();
    void runConnectProbe();
  };

  const pickLocalFolder = () => {
    void open({ directory: true, multiple: false, title: "选择本机文件夹" }).then((picked) => {
      if (typeof picked !== "string") return;
      dispatchClaude({ type: "draft-updated", draft: { localRoot: picked } });
    });
  };

  return (
    <div className="lumio-claude-sheet-back">
      <div className="lumio-claude-sheet" role="dialog" aria-labelledby="lumio-claude-connect-title">
        <ol className="lumio-claude-legend">
          <li className={current === 0 ? "is-on" : current > 0 ? "is-done" : ""}>主机</li>
          <li className={current === 1 ? "is-on" : current > 1 ? "is-done" : ""}>探测</li>
          <li className={current === 2 ? "is-on" : current > 2 ? "is-done" : ""}>装组件</li>
          <li className={current === 3 ? "is-on" : current > 3 ? "is-done" : ""}>首次同步</li>
        </ol>
        <div className="lumio-claude-sheet-steps" aria-hidden="true">
          {CONNECT_STEPS.map((step, index) => (
            <i className={index === current ? "is-on" : index < current ? "is-done" : ""} key={step} />
          ))}
        </div>

        {sheet.step === "host" ? (
          <form onSubmit={onHostSubmit}>
            <h2 id="lumio-claude-connect-title">{isNewProject ? "新建项目" : "连接服务器"}</h2>
            {isNewProject ? (
              <>
                <p className="lumio-claude-lede">在这台服务器上再建一个项目。密码和上次一样。</p>
                <p className="lumio-claude-quiet">
                  {draft.user}@{draft.host || draft.hostAlias}:{draft.port}
                </p>
                {draft.auth === "password" ? (
                  <>
                    <label className="lumio-claude-note" htmlFor="lumio-claude-pass">
                      密码
                    </label>
                    <input
                      autoComplete="off"
                      className="lumio-claude-field"
                      disabled={probing}
                      id="lumio-claude-pass"
                      onChange={(event) => onPassword(event.target.value)}
                      type="password"
                      value={password}
                    />
                    <p className="lumio-claude-quiet">密码只留在这台电脑上。</p>
                  </>
                ) : null}
              </>
            ) : null}
            {isNewProject ? null : (
            <>
            <div className="lumio-claude-mode-tabs" role="tablist" aria-label="连接方式">
              <button
                aria-selected={draft.auth !== "config"}
                className={draft.auth !== "config" ? "is-on" : ""}
                disabled={probing}
                onClick={() => dispatchClaude({ type: "draft-updated", draft: { auth: "password" } })}
                role="tab"
                type="button"
              >
                IP 用户密码
              </button>
              <button
                aria-selected={draft.auth === "config"}
                className={draft.auth === "config" ? "is-on" : ""}
                disabled={probing}
                onClick={() => dispatchClaude({ type: "draft-updated", draft: { auth: "config" } })}
                role="tab"
                type="button"
              >
                本机 SSH 方式
              </button>
            </div>
            {draft.auth === "config" ? (
              <>
                <p className="lumio-claude-lede">用本机 SSH 配置里的 Host 别名连接。密钥和端口按配置来。</p>
                <label className="lumio-claude-note" htmlFor="lumio-claude-alias">
                  本机 SSH 配置别名（Host）
                </label>
                <input
                  className="lumio-claude-field"
                  disabled={probing}
                  id="lumio-claude-alias"
                  list="lumio-claude-alias-list"
                  onChange={(event) => {
                    const hostAlias = event.target.value;
                    const known = sshHosts.find((host) => host.alias === hostAlias);
                    dispatchClaude({
                      type: "draft-updated",
                      draft: {
                        hostAlias,
                        auth: "config",
                        host: known?.hostname ?? draft.host,
                        user: known?.user ?? draft.user,
                        port: known?.port ?? draft.port,
                      },
                    });
                  }}
                  placeholder="例如 prod，读 ~/.ssh/config"
                  value={draft.hostAlias}
                />
                <datalist id="lumio-claude-alias-list">
                  {sshHosts.map((host) => (
                    <option key={host.alias} value={host.alias}>
                      {host.hostname ? `${host.alias} · ${host.hostname}` : host.alias}
                    </option>
                  ))}
                </datalist>
                <p className="lumio-claude-quiet">读这台电脑的 SSH 配置，不必再填公网 IP 和密码。</p>
              </>
            ) : (
              <>
                <p className="lumio-claude-lede">买好云服务器之后，把公网 IP 和密码填进来就行。其余默认即可。</p>
                <label className="lumio-claude-note" htmlFor="lumio-claude-host">
                  主机IP
                </label>
                <input
                  autoComplete="off"
                  className="lumio-claude-field"
                  disabled={probing}
                  id="lumio-claude-host"
                  onChange={(event) => dispatchClaude({ type: "draft-updated", draft: { host: event.target.value } })}
                  onPaste={onHostPaste}
                  value={draft.host}
                />
                <label className="lumio-claude-note" htmlFor="lumio-claude-user">
                  用户
                </label>
                <input
                  className="lumio-claude-field"
                  disabled={probing}
                  id="lumio-claude-user"
                  onChange={(event) => dispatchClaude({ type: "draft-updated", draft: { user: event.target.value } })}
                  value={draft.user}
                />
                <label className="lumio-claude-note" htmlFor="lumio-claude-pass">
                  密码
                </label>
                <input
                  autoComplete="off"
                  className="lumio-claude-field"
                  disabled={probing}
                  id="lumio-claude-pass"
                  onChange={(event) => onPassword(event.target.value)}
                  type="password"
                  value={password}
                />
                <p className="lumio-claude-quiet">
                  密码只留在这台电脑上。也可以粘贴整条 <code>ssh root@…</code>。
                </p>
              </>
            )}
            </>
            )}
            <div className="lumio-claude-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => cancelClaudeConnect()}
                type="button"
              >
                取消
              </button>
              <button
                aria-busy={probing}
                className={`lumio-button is-primary${probing ? " is-busy" : ""}`}
                disabled={probing}
                type="submit"
              >
                {probing ? <i aria-hidden="true" className="lumio-button-spinner" /> : null}
                {probing ? "正在探测…" : "探测连接"}
              </button>
            </div>
          </form>
        ) : null}

        {sheet.step === "probe" && sheet.probeStatus !== "fail" ? (
          <div>
            <h2 id="lumio-claude-connect-title">探测连接</h2>
            <p className="lumio-claude-lede">正在确认这台机器能登录、能跑组件。</p>
            <div className="lumio-claude-checks">
              <CheckRow
                done={sheet.probe?.reachable === true}
                now={sheet.probeStatus === "running" && sheet.probe === null}
                title="网络可达"
                detail={`${draft.host || draft.hostAlias || "—"}:${draft.port}`}
              />
              <CheckRow
                done={sheet.probe?.authenticated === true}
                now={sheet.probeStatus === "running" && sheet.probe?.reachable === true}
                title="可以登录"
                detail={`${draft.user} · ${draft.auth === "config" ? "本机 SSH 配置" : draft.auth === "key" ? "密钥" : "密码有效"}`}
              />
              <CheckRow
                done={sheet.probeStatus === "ok"}
                now={sheet.probeStatus === "running" && sheet.probe?.authenticated === true}
                title="系统可用"
                detail={
                  sheet.probe?.distro
                    ? `${sheet.probe.distro}${sheet.probe.cpu ? ` · ${sheet.probe.cpu} 核` : ""}${sheet.probe.memory ? ` · ${sheet.probe.memory}` : ""}`
                    : "发行版 / CPU / 内存"
                }
              />
            </div>
            {sheet.probeStatus === "ok" ? (
              <>
                <p className="lumio-claude-lede">
                  {isNewProject
                    ? "在这台服务器上再建一个项目。选好本机和服务器上的文件夹，再装组件。"
                    : "探测通过。选好本机和服务器上的项目目录，再装组件。"}
                </p>
                <label className="lumio-claude-note" htmlFor="lumio-claude-local-root">
                  本机文件夹
                </label>
                <div className="lumio-claude-path-row">
                  <input
                    className="lumio-claude-field"
                    id="lumio-claude-local-root"
                    onChange={(event) =>
                      dispatchClaude({ type: "draft-updated", draft: { localRoot: event.target.value } })
                    }
                    value={local}
                  />
                  <button className="lumio-button is-secondary" onClick={pickLocalFolder} type="button">
                    选择
                  </button>
                </div>
                <label className="lumio-claude-note" htmlFor="lumio-claude-remote-root">
                  服务器文件夹
                </label>
                <input
                  className="lumio-claude-field"
                  id="lumio-claude-remote-root"
                  onChange={(event) => {
                    const remoteRoot = event.target.value;
                    dispatchClaude({
                      type: "draft-updated",
                      draft: { remoteRoot, projectName: folderNameFromPath(remoteRoot) },
                    });
                  }}
                  value={remote}
                />
                <p className="lumio-claude-quiet">服务器上会建这个目录，并和本机文件夹双向同步。</p>
              </>
            ) : null}
            <div className="lumio-claude-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => dispatchClaude({ type: "back-to-host" })}
                type="button"
              >
                返回修改
              </button>
              <button
                className="lumio-button is-primary"
                disabled={sheet.probeStatus !== "ok" || !foldersReady}
                onClick={() => void runConnectSetup()}
                type="button"
              >
                继续装组件
              </button>
            </div>
          </div>
        ) : null}

        {sheet.step === "probe" && sheet.probeStatus === "fail" ? (
          <div>
            <h2 id="lumio-claude-connect-title">连不上这台服务器</h2>
            <p className="lumio-claude-lede">先对一下这三件，大多数情况是其中一件。</p>
            <div className="lumio-claude-fail">
              {sheet.probe?.detail ?? probeErrorCopy(sheet.probe?.errorCode ?? "SSH_AUTH_FAILED", draft.host, draft.port)}
              <span className="lumio-claude-fail-code">{sheet.probe?.errorCode ?? "SSH_AUTH_FAILED"}</span>
            </div>
            <div className="lumio-claude-promise">
              {TROUBLESHOOTING.map((item) => (
                <div key={item.title}>
                  <b>{item.title}</b>
                  <span>{item.detail}</span>
                </div>
              ))}
            </div>
            <div className="lumio-claude-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => dispatchClaude({ type: "back-to-host" })}
                type="button"
              >
                返回修改
              </button>
              <button
                aria-busy={probing}
                className={`lumio-button is-primary${probing ? " is-busy" : ""}`}
                disabled={probing}
                onClick={() => void runConnectProbe()}
                type="button"
              >
                {probing ? <i aria-hidden="true" className="lumio-button-spinner" /> : null}
                {probing ? "正在探测…" : "重试"}
              </button>
            </div>
          </div>
        ) : null}

        {sheet.step === "setup" && sheet.setupStatus === "choose" && sheet.rootChoice ? (
          <div className="lumio-claude-choice" role="dialog" aria-modal="true" aria-labelledby="lumio-claude-connect-title">
            <h2 id="lumio-claude-connect-title">服务器上已有这个项目</h2>
            <p className="lumio-claude-lede">
              发现 {sheet.rootChoice.existingRoot}。继续用这里的文件，还是另外建一个新目录？
            </p>
            <div className="lumio-claude-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => void runConnectSetup("use")}
                type="button"
              >
                继续使用
              </button>
              <button
                className="lumio-button is-primary"
                onClick={() => void runConnectSetup("create")}
                type="button"
              >
                新建 {sheet.rootChoice.nextName}
              </button>
            </div>
          </div>
        ) : null}

        {sheet.step === "setup" && sheet.setupStatus !== "fail" && sheet.setupStatus !== "choose" ? (
          <div>
            <h2 id="lumio-claude-connect-title">安装组件</h2>
            <p className="lumio-claude-lede">在服务器上准备同步环境和项目目录。不用你操作。</p>
            <p className="lumio-claude-meta">
              {sheet.setupStatus === "ok"
                ? "安装完成"
                : `${sheet.setupProgress ? `第 ${sheet.setupProgress.step} / ${sheet.setupProgress.total} 步 · ` : ""}${sheet.setupProgress?.detail ?? setupPhaseCopy("inspect")} · ${formatSetupElapsed(setupElapsed)}`}
            </p>
            <div
              className={`lumio-claude-progress${installing && (sheet.setupProgress?.phase === "upload" || sheet.setupProgress?.phase === "inspect") ? " is-indeterminate" : ""}`}
            >
              <i
                style={{
                  width:
                    sheet.setupStatus === "ok"
                      ? "100%"
                      : `${Math.max(8, Math.round(((sheet.setupProgress?.step ?? 1) / (sheet.setupProgress?.total ?? 4)) * 100))}%`,
                }}
              />
            </div>
            <div className="lumio-claude-checks">
              <CheckRow done title="已连上服务器" detail={`${draft.user}@${draft.host || draft.hostAlias}`} />
              <CheckRow
                done={sheet.setupStatus === "ok"}
                now={installing && sheet.setupProgress?.phase !== "mkdir"}
                title="安装同步组件"
                detail={
                  sheet.setupStatus === "ok"
                    ? "已传到服务器"
                    : sheet.setupProgress?.phase === "mkdir"
                      ? "等待上传"
                      : (sheet.setupProgress?.detail ?? "正在准备…")
                }
              />
              <CheckRow
                done={sheet.setupStatus === "ok" || sheet.setupProgress?.phase === "upload" || sheet.setupProgress?.phase === "finish"}
                now={installing && sheet.setupProgress?.phase === "mkdir"}
                title="创建项目目录"
                detail={`${remote} · 本机 ${local}`}
              />
            </div>
            <div className="lumio-claude-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => cancelClaudeConnect()}
                type="button"
              >
                取消
              </button>
              <button
                aria-busy={installing}
                className={`lumio-button is-primary${installing ? " is-busy" : ""}`}
                disabled={sheet.setupStatus !== "ok"}
                onClick={() => void runConnectSync()}
                type="button"
              >
                {installing ? <i aria-hidden="true" className="lumio-button-spinner" /> : null}
                {installing ? "正在安装…" : "开始首次同步"}
              </button>
            </div>
          </div>
        ) : null}

        {sheet.step === "setup" && sheet.setupStatus === "fail" ? (
          sheet.setupErrorCode === "DEPLOY_ARTIFACT_MISSING" ? (
            <div>
              <h2 id="lumio-claude-connect-title">这个版本没有打进同步组件</h2>
              <p className="lumio-claude-lede">不是连接问题，不用改服务器信息。更新或重装 BestCodex 后回到这里重试。</p>
              <div className="lumio-claude-fail">
                {sheet.setupDetail ?? prepareErrorCopy("DEPLOY_ARTIFACT_MISSING", draft.host, draft.port)}
                <span className="lumio-claude-fail-code">DEPLOY_ARTIFACT_MISSING</span>
              </div>
              <div className="lumio-claude-actions">
                <button className="lumio-button is-secondary" onClick={() => void openInBrowser(HELP_URL)} type="button">
                  打开帮助页
                </button>
                <button className="lumio-button is-primary" onClick={() => void runConnectSetup()} type="button">
                  重试
                </button>
              </div>
            </div>
          ) : (
            <div>
              <h2 id="lumio-claude-connect-title">没能装好同步组件</h2>
              <p className="lumio-claude-lede">先改连接信息，或再试一次。装不好就不能开始首次同步。</p>
              <div className="lumio-claude-fail">
                {sheet.setupDetail ?? prepareErrorCopy("SSH_PREPARE_FAILED", draft.host, draft.port)}
                <span className="lumio-claude-fail-code">{sheet.setupErrorCode ?? "SSH_PREPARE_FAILED"}</span>
              </div>
              <div className="lumio-claude-actions">
                <button
                  className="lumio-button is-secondary"
                  onClick={() => dispatchClaude({ type: "back-to-host" })}
                  type="button"
                >
                  返回修改
                </button>
                <button className="lumio-button is-primary" onClick={() => void runConnectSetup()} type="button">
                  重试
                </button>
              </div>
            </div>
          )
        ) : null}

        {sheet.step === "sync" && sheet.sync.state === "fail" ? (
          <div>
            <h2 id="lumio-claude-connect-title">没能完成首次同步</h2>
            <p className="lumio-claude-lede">文件还没拉到这台电脑。先改连接信息，或再试一次。</p>
            <div className="lumio-claude-fail">
              {syncErrorCopy(sheet.sync.errorCode)}
              <span className="lumio-claude-fail-code">{sheet.sync.errorCode ?? "SYNC_FAILED"}</span>
            </div>
            <div className="lumio-claude-actions">
              <button
                className="lumio-button is-secondary"
                onClick={() => dispatchClaude({ type: "back-to-host" })}
                type="button"
              >
                返回修改
              </button>
              <button className="lumio-button is-primary" onClick={() => void runConnectSync()} type="button">
                重试
              </button>
            </div>
          </div>
        ) : null}

        {sheet.step === "sync" && sheet.sync.state !== "fail" ? (
          <div>
            <h2 id="lumio-claude-connect-title">首次同步</h2>
            <p className="lumio-claude-lede">把服务器上的项目拉到这台电脑。完成后右侧就是终端。</p>
            <p className="lumio-claude-meta">
              {sheet.sync.filesTotal > 0
                ? `${sheet.sync.filesDone} / ${sheet.sync.filesTotal} 个文件 · ${draft.projectName}`
                : `准备本机目录 · ${draft.projectName}`}
            </p>
            <div className={`lumio-claude-progress${sheet.sync.filesTotal === 0 ? " is-indeterminate" : ""}`}>
              <i
                style={{
                  width:
                    sheet.sync.filesTotal > 0
                      ? `${Math.max(6, Math.round((sheet.sync.filesDone / sheet.sync.filesTotal) * 100))}%`
                      : "36%",
                }}
              />
            </div>
            <div className="lumio-claude-checks">
              <CheckRow done title="目录已创建" />
              <CheckRow now={sheet.sync.state === "running"} done={sheet.sync.state === "ok"} title="正在同步文件" />
              <CheckRow title="打开 Claude 终端" />
            </div>
            <div className="lumio-claude-actions">
              <button className="lumio-button is-secondary" onClick={onBackToCodex} type="button">
                切到 Codex（同步继续）
              </button>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}

function CheckRow({
  title,
  detail,
  done,
  now,
}: {
  title: string;
  detail?: string;
  done?: boolean;
  now?: boolean;
}) {
  return (
    <div className={`lumio-claude-check${done ? " is-done" : now ? " is-now" : ""}`}>
      <i />
      <div>
        {title}
        {detail ? <span>{detail}</span> : null}
      </div>
    </div>
  );
}
