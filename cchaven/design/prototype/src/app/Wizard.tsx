import { useEffect, useMemo, useState } from "react";
import { Project, sleep, useDemo } from "../demo";

const STEPS = ["连接服务器", "项目设置", "完成"];

type TestState = "idle" | "testing" | "ok" | "fail";
type StageState = "pending" | "running" | "done" | "failed";

/** 解析粘贴的 ssh 命令或 user@host，方便小白直接从云厂商文档复制 */
function parseSshPaste(text: string): { host: string; user?: string; port?: string } | null {
  const t = text.trim();
  // ssh root@1.2.3.4 -p 2222  /  ssh -p 2222 root@1.2.3.4
  const m = t.match(/^ssh\s+(?:-p\s*(\d+)\s+)?(?:([\w.-]+)@)?([\w.-]+)(?:\s+-p\s*(\d+))?$/i);
  if (m) return { host: m[3], user: m[2], port: m[1] || m[4] };
  // root@1.2.3.4
  const m2 = t.match(/^([\w.-]+)@([\w.-]+)$/);
  if (m2) return { host: m2[2], user: m2[1] };
  return null;
}

export default function Wizard({
  onDone,
  onCancel,
}: {
  onDone: (p: Project) => void;
  onCancel: () => void;
}) {
  const { showErrors, toast } = useDemo();
  const [step, setStep] = useState(0);
  const [err, setErr] = useState("");

  /* 第 1 步：连接服务器 */
  const [addr, setAddr] = useState("");
  const [user, setUser] = useState("root");
  const [pw, setPw] = useState("");
  const [port, setPort] = useState("22");
  const [authKey, setAuthKey] = useState(false); // 高级：改用 SSH 密钥
  const [test, setTest] = useState<TestState>("idle");

  /* 第 2 步：项目设置 */
  const [name, setName] = useState("");
  const [remoteEdited, setRemoteEdited] = useState(false);
  const [remoteRoot, setRemoteRoot] = useState("");
  const [localEdited, setLocalEdited] = useState(false);
  const [localRoot, setLocalRoot] = useState("");
  const [excludes, setExcludes] = useState(".git/\nnode_modules/\ntarget/\n.env");

  /* 远端目录按登录用户自动预设：root → /root/…，其他 → /home/<user>/… */
  const presetRemote = useMemo(() => {
    const base = user === "root" ? "/root" : `/home/${user || "user"}`;
    return `${base}/cchaven/${name || "my-project"}`;
  }, [user, name]);
  const presetLocal = `/Users/mary/CCHaven/${name || "my-project"}`;
  const effRemote = remoteEdited ? remoteRoot : presetRemote;
  const effLocal = localEdited ? localRoot : presetLocal;

  /* 第 3 步：部署 */
  const [deploying, setDeploying] = useState(false);
  const [stages, setStages] = useState<StageState[]>(["pending", "pending", "pending", "pending"]);
  const [deployErr, setDeployErr] = useState("");
  const [syncCount, setSyncCount] = useState(0);
  const stageLabels = [
    "连接服务器",
    "安装 CC避风港同步组件（自动完成，无需操作）",
    `创建项目目录 ${effRemote}`,
    syncCount > 0 ? `首次同步（${syncCount}/456 个文件）` : "首次同步并启动 Claude Code 终端",
  ];

  useEffect(() => {
    function esc(e: KeyboardEvent) {
      if (e.key === "Escape" && !deploying) onCancel();
    }
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [deploying, onCancel]);

  function handleAddrPaste(e: React.ClipboardEvent<HTMLInputElement>) {
    const parsed = parseSshPaste(e.clipboardData.getData("text"));
    if (!parsed) return;
    e.preventDefault();
    setAddr(parsed.host);
    if (parsed.user) setUser(parsed.user);
    if (parsed.port) setPort(parsed.port);
    setTest("idle");
    toast("已自动识别服务器地址" + (parsed.user ? "和用户名" : "") + "。");
  }

  /** 第 1 步的主按钮：自动测试连接，成功即进入下一步 */
  async function connectAndNext() {
    setErr("");
    if (!addr) return setErr("请填写服务器 IP 地址。");
    if (!authKey && !pw) return setErr("请填写密码（买服务器时设置的 root 密码）。");
    setTest("testing");
    await sleep(1400);
    if (showErrors) {
      setTest("fail");
      return;
    }
    setTest("ok");
    await sleep(500);
    setStep(1);
  }

  function next() {
    setErr("");
    if (step === 1) {
      if (!name.trim()) return setErr("给项目起个名字，例如 my-project。");
      if (remoteEdited && !remoteRoot.startsWith("/")) return setErr("远端目录必须以 / 开头。");
      setStep(2);
      return;
    }
    setStep((s) => Math.min(s + 1, 2));
  }

  async function deploy(fromStage = 0) {
    setDeploying(true);
    setDeployErr("");
    const st = [...stages];
    for (let i = fromStage; i < 4; i++) {
      st[i] = "running";
      setStages([...st]);
      if (i === 3) {
        for (let n = 0; n <= 456; n += 57) {
          setSyncCount(Math.min(n, 456));
          await sleep(150);
        }
        setSyncCount(456);
      } else {
        await sleep(900);
      }
      if (i === 1 && showErrors && fromStage === 0) {
        st[i] = "failed";
        setStages([...st]);
        setDeployErr("服务器磁盘空间不足（剩余 120 MB）。请清理磁盘后点击重试，或联系客服协助。");
        setDeploying(false);
        return;
      }
      st[i] = "done";
      setStages([...st]);
    }
    await sleep(400);
    onDone({
      id: crypto.randomUUID(),
      name: name.trim(),
      host: `${user}@${addr}`,
      remoteRoot: effRemote,
      localRoot: effLocal,
      status: "synced",
    });
  }

  const failedAt = stages.indexOf("failed");

  return (
    <div className="modal-backdrop" onMouseDown={(e) => { if (e.target === e.currentTarget && !deploying) onCancel(); }}>
      <div className="modal">
        <h2 style={{ fontSize: 20 }}>新建项目</h2>
        <div className="wizard-steps">
          {STEPS.map((s, i) => (
            <div key={s} className={i <= step ? "on" : ""}>
              {i + 1}. {s}
            </div>
          ))}
        </div>

        {step === 0 && (
          <>
            <div className="helper-box">
              💡 在阿里云、腾讯云、AWS 等平台买好服务器后，你会得到一个 <strong>IP 地址</strong>和
              <strong> root 密码</strong>，填到下面就行，其余都交给我们。
              <a href="#" onClick={(e) => { e.preventDefault(); toast("原型：打开图文教程《3 分钟买一台能跑 Claude Code 的服务器》。"); }}>
                还没有服务器？看购买教程 ↗
              </a>
            </div>
            <div className="field">
              <label>服务器 IP 地址</label>
              <input
                value={addr}
                placeholder="例如 43.156.20.8"
                onChange={(e) => { setAddr(e.target.value); setTest("idle"); }}
                onPaste={handleAddrPaste}
                disabled={test === "testing"}
              />
              <div className="hint">支持直接粘贴整条 ssh 命令（如 ssh root@43.156.20.8），会自动识别。</div>
            </div>
            <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 12 }}>
              <div className="field">
                <label>用户名</label>
                <input value={user} onChange={(e) => { setUser(e.target.value); setTest("idle"); }} disabled={test === "testing"} />
                <div className="hint">新买的服务器一般就是 root。</div>
              </div>
              {!authKey && (
                <div className="field">
                  <label>密码</label>
                  <input
                    type="password"
                    value={pw}
                    placeholder="买服务器时设置的密码"
                    onChange={(e) => { setPw(e.target.value); setTest("idle"); }}
                    disabled={test === "testing"}
                  />
                  <div className="hint">密码只保存在你自己的电脑上（系统钥匙串加密）。</div>
                </div>
              )}
            </div>

            <details className="advanced">
              <summary>高级选项（懂 SSH 的用户使用）</summary>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 2fr", gap: 12, marginTop: 12 }}>
                <div className="field">
                  <label>端口</label>
                  <input value={port} onChange={(e) => setPort(e.target.value)} />
                </div>
                <div className="field">
                  <label>认证方式</label>
                  <select value={authKey ? "key" : "pw"} onChange={(e) => setAuthKey(e.target.value === "key")}>
                    <option value="pw">密码（默认）</option>
                    <option value="key">SSH 密钥（~/.ssh/id_ed25519）</option>
                  </select>
                  <div className="hint">也可从 ~/.ssh/config 选择已有主机。</div>
                </div>
              </div>
            </details>

            {test === "ok" && (
              <div className="banner ok" style={{ marginTop: 14, marginBottom: 0 }}>
                ✓ 连接成功！服务器环境正常（Ubuntu 24.04），正在进入下一步…
              </div>
            )}
            {test === "fail" && (
              <div className="banner error" style={{ marginTop: 14, marginBottom: 0, display: "block" }}>
                <strong>连不上服务器，请按顺序检查：</strong>
                <ol style={{ margin: "8px 0 0 18px", lineHeight: 1.7 }}>
                  <li>IP 地址是否照着云平台控制台抄对了（公网 IP，不是内网 IP）；</li>
                  <li>密码是否正确（注意大小写，建议重新复制粘贴）；</li>
                  <li>云平台的「安全组 / 防火墙」是否放行了 {port} 端口。</li>
                </ol>
                <a href="#" style={{ display: "inline-block", marginTop: 8 }} onClick={(e) => { e.preventDefault(); toast("原型：打开常见连接问题图文排查页。"); }}>
                  查看图文排查教程 ↗
                </a>
              </div>
            )}
          </>
        )}

        {step === 1 && (
          <>
            <div className="field">
              <label>项目名称</label>
              <input value={name} placeholder="my-project" onChange={(e) => setName(e.target.value)} autoFocus />
              <div className="hint">随便起，之后可以改。</div>
            </div>

            <div className="field">
              <label>服务器上的项目目录</label>
              {!remoteEdited ? (
                <div className="preset-row">
                  <code>{presetRemote}</code>
                  <span className="tag-auto">已自动设置</span>
                  <button type="button" className="btn btn-ghost" style={{ padding: "2px 8px", fontSize: 12.5 }} onClick={() => { setRemoteEdited(true); setRemoteRoot(presetRemote); }}>
                    修改
                  </button>
                </div>
              ) : (
                <div style={{ display: "flex", gap: 8 }}>
                  <input value={remoteRoot} onChange={(e) => setRemoteRoot(e.target.value)} />
                  <button type="button" className="btn btn-secondary" onClick={() => setRemoteEdited(false)}>用推荐值</button>
                </div>
              )}
              <div className="hint">目录不存在会自动创建，你不需要懂 Linux 路径。</div>
            </div>

            <div className="field">
              <label>电脑上的同步文件夹</label>
              {!localEdited ? (
                <div className="preset-row">
                  <code>{presetLocal}</code>
                  <span className="tag-auto">已自动设置</span>
                  <button type="button" className="btn btn-ghost" style={{ padding: "2px 8px", fontSize: 12.5 }} onClick={() => { setLocalEdited(true); setLocalRoot(presetLocal); }}>
                    修改
                  </button>
                </div>
              ) : (
                <div style={{ display: "flex", gap: 8 }}>
                  <input value={localRoot} onChange={(e) => setLocalRoot(e.target.value)} />
                  <button type="button" className="btn btn-secondary" onClick={() => setLocalEdited(false)}>用推荐值</button>
                </div>
              )}
              <div className="hint">服务器上的文件会实时同步到这里，用你熟悉的软件随时打开。</div>
            </div>

            <details className="advanced">
              <summary>高级选项：同步排除规则</summary>
              <div className="field" style={{ marginTop: 12 }}>
                <label>不同步的内容（每行一条，已按最佳实践预设）</label>
                <textarea rows={4} value={excludes} onChange={(e) => setExcludes(e.target.value)} />
              </div>
              <div className="banner ok" style={{ marginBottom: 0 }}>
                🛡 机密文件（.env、密钥）默认受保护，永不同步。
              </div>
            </details>
          </>
        )}

        {step === 2 && (
          <>
            <table style={{ fontSize: 14, borderSpacing: "0 6px", marginBottom: 8 }}>
              <tbody>
                <tr><td style={{ color: "var(--gray)", paddingRight: 18 }}>项目</td><td><strong>{name}</strong></td></tr>
                <tr><td style={{ color: "var(--gray)" }}>服务器</td><td>{user}@{addr} <span style={{ color: "var(--green)", fontSize: 12.5 }}>✓ 已验证连接</span></td></tr>
                <tr><td style={{ color: "var(--gray)" }}>服务器目录</td><td style={{ fontFamily: "monospace", fontSize: 13 }}>{effRemote}</td></tr>
                <tr><td style={{ color: "var(--gray)" }}>本地文件夹</td><td style={{ fontFamily: "monospace", fontSize: 13 }}>{effLocal}</td></tr>
              </tbody>
            </table>

            {(deploying || stages.some((s) => s !== "pending")) && (
              <div style={{ margin: "14px 0 4px" }}>
                {stageLabels.map((label, i) => (
                  <div className="deploy-stage" key={i}>
                    <span className="st">
                      {stages[i] === "running" && <span className="spinner dark" style={{ margin: 0 }} />}
                      {stages[i] === "done" && <span style={{ color: "var(--green)" }}>✓</span>}
                      {stages[i] === "failed" && <span style={{ color: "var(--red)" }}>✗</span>}
                      {stages[i] === "pending" && <span style={{ color: "#bbb" }}>○</span>}
                    </span>
                    <span style={{ color: stages[i] === "pending" ? "#999" : undefined }}>{label}</span>
                  </div>
                ))}
                {deployErr && <div className="banner error" style={{ marginTop: 10, marginBottom: 0 }}>{deployErr}</div>}
              </div>
            )}
            {!deploying && stages.every((s) => s === "pending") && (
              <p style={{ color: "var(--gray)", fontSize: 13.5, marginTop: 8 }}>
                点「完成设置」后一切自动进行，大约需要 1 分钟。
              </p>
            )}
          </>
        )}

        {err && <p style={{ color: "var(--red)", fontSize: 13, marginTop: 12 }}>{err}</p>}

        <div className="wizard-nav">
          {step > 0 ? (
            <button className="btn btn-secondary" onClick={() => { setErr(""); setStep(step - 1); }} disabled={deploying}>
              上一步
            </button>
          ) : (
            <button className="btn btn-secondary" onClick={onCancel} disabled={test === "testing"}>取消</button>
          )}
          {step === 0 && (
            <button className="btn btn-primary" onClick={connectAndNext} disabled={test === "testing"}>
              {test === "testing" && <span className="spinner" />}
              {test === "testing" ? "正在连接…" : "连接并继续"}
            </button>
          )}
          {step === 1 && (
            <button className="btn btn-primary" onClick={next}>下一步</button>
          )}
          {step === 2 && (failedAt >= 0 ? (
            <button className="btn btn-primary" onClick={() => { const st = [...stages]; st[failedAt] = "pending"; setStages(st); deploy(failedAt); }}>
              重试
            </button>
          ) : (
            <button className="btn btn-primary" onClick={() => deploy(0)} disabled={deploying}>
              {deploying && <span className="spinner" />}
              {deploying ? "设置中…" : "完成设置"}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
