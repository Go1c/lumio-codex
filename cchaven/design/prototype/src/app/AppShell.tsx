import { useEffect, useRef, useState } from "react";
import { Project, sleep, useDemo } from "../demo";
import Wizard from "./Wizard";
import Workspace from "./Workspace";

type MainView = { kind: "empty" } | { kind: "workspace"; id: string };

export default function AppShell() {
  const { authed } = useDemo();
  if (!authed) return <AppLogin />;
  return <Shell />;
}

/* ---------- APP 登录：打开浏览器授权（无密码输入） ---------- */

function AppLogin() {
  const { showErrors, setAuthed, setEmail, setProjects, invited, toast } = useDemo();
  const [phase, setPhase] = useState<"idle" | "waiting" | "timeout">("idle");

  async function startAuth() {
    setPhase("waiting");
    await sleep(2600);
    if (showErrors) {
      setPhase("timeout");
      return;
    }
    setEmail("mary@example.com");
    setProjects([]);
    setAuthed(true);
    if (invited) toast("🎁 首月免费试用已开通，有效期至 2026年9月11日。");
  }

  return (
    <div className="auth-page" style={{ minHeight: "100vh", background: "#26282e" }}>
      <div className="auth-card">
        <div className="logo" style={{ justifyContent: "center", marginBottom: 18 }}>
          <span className="mark" /> CC避风港 <span style={{ fontWeight: 400, color: "var(--gray)", fontSize: 13 }}>CCHaven</span>
        </div>

        {phase === "idle" && (
          <>
            <h2>登录</h2>
            <p className="sub">点击下方按钮，在浏览器中登录并授权本应用。</p>
            <button className="btn btn-primary" onClick={startAuth}>
              通过浏览器登录 ↗
            </button>
            <div className="terms" style={{ marginTop: 18 }}>
              将打开系统浏览器跳转到 cchaven.cn。没有账号？在浏览器里可以直接注册。
              <br />应用本身不收集你的密码。
            </div>
          </>
        )}

        {phase === "waiting" && (
          <>
            <h2>等待浏览器授权…</h2>
            <p className="sub">
              已在浏览器中打开登录页。完成登录并点击「授权」后，这里会自动进入。
            </p>
            <div style={{ margin: "6px 0 22px" }}>
              <span className="spinner dark" style={{ width: 22, height: 22 }} />
            </div>
            <div style={{ display: "flex", gap: 10, justifyContent: "center" }}>
              <button className="btn btn-secondary" onClick={() => toast("原型：重新打开了浏览器登录页。")}>
                重新打开浏览器
              </button>
              <button className="btn btn-ghost" onClick={() => setPhase("idle")}>取消</button>
            </div>
            <div className="terms" style={{ marginTop: 18 }}>原型演示：约 2 秒后自动完成授权。</div>
          </>
        )}

        {phase === "timeout" && (
          <>
            <h2>授权未完成</h2>
            <div className="banner error" style={{ marginTop: 14 }}>
              <span>等待授权超时。浏览器可能没有打开，或你尚未在浏览器中完成登录。</span>
            </div>
            <button className="btn btn-primary" onClick={startAuth}>重试</button>
            <div className="terms" style={{ marginTop: 16 }}>
              一直失败？检查默认浏览器设置，或将浏览器地址栏中的授权码手动粘贴到这里（原型未展开）。
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/* ---------- 主外壳与侧栏 ---------- */

const STATUS_META: Record<Project["status"], { label: string; cls: string }> = {
  synced: { label: "已全部同步", cls: "synced" },
  syncing: { label: "正在同步 3 个文件…", cls: "syncing" },
  conflicts: { label: "2 个冲突", cls: "conflicts" },
  offline: { label: "离线 — 5 秒后重试", cls: "offline" },
};

function Shell() {
  const { email, projects, setProjects, setAuthed, toast, invited } = useDemo();
  const [view, setView] = useState<MainView>({ kind: "empty" });
  const [wizardOpen, setWizardOpen] = useState(false);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [acctOpen, setAcctOpen] = useState(false);
  const acctRef = useRef<HTMLDivElement>(null);
  const [globalStatus, setGlobalStatus] = useState<Project["status"]>("synced");

  useEffect(() => {
    if (!acctOpen) return;
    function onDoc(e: MouseEvent) {
      if (!acctRef.current?.contains(e.target as Node)) setAcctOpen(false);
    }
    window.addEventListener("mousedown", onDoc);
    return () => window.removeEventListener("mousedown", onDoc);
  }, [acctOpen]);

  const activeId = view.kind === "workspace" ? view.id : null;

  function addProject(p: Project) {
    setProjects([...projects, p]);
    setWizardOpen(false);
    setView({ kind: "workspace", id: p.id });
  }

  function deleteProject(id: string) {
    if (!confirm("确定要从 CC避风港移除该项目吗？\n\n不会删除任何本地或远端文件。")) return;
    setProjects(projects.filter((p) => p.id !== id));
    if (activeId === id) setView({ kind: "empty" });
    toast("已移除项目。");
  }

  const meta = STATUS_META[globalStatus];

  return (
    <div className="appframe">
      <aside className="app-sidebar">
        <h2>项目</h2>
        {projects.length === 0 && (
          <div style={{ padding: "4px 10px", fontSize: 13, color: "#8b8f98" }}>还没有项目。</div>
        )}
        {projects.map((p) => (
          <div
            key={p.id}
            className={`proj-item ${activeId === p.id ? "active" : ""}`}
            onClick={() => setView({ kind: "workspace", id: p.id })}
          >
            <span className={`dot ${p.status}`} />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p.name}</span>
            <button
              className="menu-btn"
              onClick={(e) => { e.stopPropagation(); setMenuFor(menuFor === p.id ? null : p.id); }}
            >
              …
            </button>
            {menuFor === p.id && (
              <div
                style={{
                  position: "absolute", right: 6, top: 34, background: "#fff", color: "#222",
                  borderRadius: 8, boxShadow: "0 8px 24px rgba(0,0,0,.35)", zIndex: 50,
                  fontSize: 13, overflow: "hidden", minWidth: 180,
                }}
                onClick={(e) => e.stopPropagation()}
              >
                {[
                  ["编辑…", () => { setMenuFor(null); toast("原型：编辑会打开预填的向导。"); }],
                  ["在 Finder 中显示", () => { setMenuFor(null); toast(`正在 Finder 中显示 ${p.localRoot}…`); }],
                  ["删除…", () => { setMenuFor(null); deleteProject(p.id); }],
                ].map(([label, fn]) => (
                  <div
                    key={label as string}
                    style={{ padding: "9px 14px", cursor: "pointer", color: label === "删除…" ? "var(--red)" : undefined }}
                    onMouseDown={fn as () => void}
                    onMouseOver={(e) => (e.currentTarget.style.background = "#f1f3f6")}
                    onMouseOut={(e) => (e.currentTarget.style.background = "")}
                  >
                    {label as string}
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
        <button className="btn btn-primary" style={{ marginTop: 10 }} onClick={() => setWizardOpen(true)}>
          + 新建项目
        </button>

        <div className="sidebar-bottom">
          <div
            className="sync-bar"
            title="点击切换状态（原型演示）"
            onClick={() => {
              const order: Project["status"][] = ["synced", "syncing", "conflicts", "offline"];
              setGlobalStatus(order[(order.indexOf(globalStatus) + 1) % 4]);
            }}
          >
            <span className={`dot ${meta.cls}`} />
            {meta.label}
          </div>
          <div ref={acctRef} style={{ position: "relative" }}>
            {acctOpen && (
              <div className="acct-menu">
                <div className="acct-menu-head">
                  <span className="avatar">{email[0]?.toUpperCase() ?? "U"}</span>
                  <div style={{ minWidth: 0 }}>
                    <div style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{email}</div>
                    <div className="plan-line">
                      {invited ? "免费试用中 · 剩余 23 天" : "已订阅 · 有效期至 2026年9月8日（剩余 27 天）"}
                    </div>
                  </div>
                </div>
                {(
                  [
                    ["🌐", "管理订阅与账号 ↗", "原型：打开官网账户中心（cchaven.cn/account）。"],
                    ["🎁", "邀请好友 ↗", "原型：打开官网账户中心的邀请分区。"],
                    ["📖", "使用文档 ↗", "原型：打开官网文档。"],
                    ["💬", "联系我们 ↗", "原型：打开客服会话。"],
                  ] as [string, string, string][]
                ).map(([ic, label, msg]) => (
                  <div key={label} className="acct-menu-item" onClick={() => { setAcctOpen(false); toast(msg); }}>
                    <span className="ic">{ic}</span>
                    {label}
                  </div>
                ))}
                <div className="acct-menu-sep" />
                <div className="acct-menu-item" onClick={() => setAuthed(false)}>
                  <span className="ic">↩</span>
                  退出登录
                </div>
              </div>
            )}
            <div className="account-chip" onClick={() => setAcctOpen(!acctOpen)}>
              <span className="avatar">{email[0]?.toUpperCase() ?? "U"}</span>
              <div style={{ minWidth: 0, lineHeight: 1.3 }}>
                <div style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{email}</div>
                <div style={{ fontSize: 11, color: "#8b8f98" }}>{invited ? "免费试用 · 剩 23 天" : "已订阅 · 剩 27 天"}</div>
              </div>
            </div>
          </div>
        </div>
      </aside>

      <main className="app-main">
        {view.kind === "empty" && (
          <div className="empty-state">
            <div className="art">🖥️ ⇄ ☁️</div>
            <h3>把你的云服务器变成 Claude Code 工作台</h3>
            <p style={{ maxWidth: 420, textAlign: "center", fontSize: 14 }}>
              只需要服务器的 IP 地址和密码，3 分钟完成设置。
              没有服务器也没关系，向导里有购买教程。
            </p>
            <button className="btn btn-primary btn-lg" onClick={() => setWizardOpen(true)}>
              + 新建项目
            </button>
          </div>
        )}
        {view.kind === "workspace" && <Workspace project={projects.find((p) => p.id === view.id)!} />}
      </main>

      {wizardOpen && <Wizard onDone={addProject} onCancel={() => setWizardOpen(false)} />}
    </div>
  );
}
