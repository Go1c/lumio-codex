import { useEffect, useRef, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { DEMO_CODE, sleep, useDemo } from "../demo";

/* ---------- 共用组件 ---------- */

export function PasswordField({
  label = "密码",
  value,
  onChange,
  showRules = true,
}: {
  label?: string;
  value: string;
  onChange: (v: string) => void;
  showRules?: boolean;
}) {
  const [visible, setVisible] = useState(false);
  const hasLen = value.length >= 8;
  const hasMix = /[a-zA-Z]/.test(value) && /\d/.test(value);
  const level = (hasLen ? 1 : 0) + (hasMix ? 1 : 0) + (value.length >= 12 && hasMix ? 1 : 0);
  return (
    <div className="field">
      <label>{label}</label>
      <div style={{ position: "relative" }}>
        <input
          type={visible ? "text" : "password"}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="••••••••"
        />
        <button
          type="button"
          onClick={() => setVisible(!visible)}
          aria-label={visible ? "隐藏密码" : "显示密码"}
          style={{
            position: "absolute", right: 10, top: "50%", transform: "translateY(-50%)",
            background: "none", border: "none", fontSize: 15,
          }}
        >
          {visible ? "🙈" : "👁"}
        </button>
      </div>
      {showRules && value.length > 0 && (
        <>
          <div className={`strength s${level}`}>
            <span /><span /><span />
          </div>
          <ul className="rules">
            <li className={hasLen ? "ok" : ""}>{hasLen ? "✓" : "○"} 至少 8 个字符</li>
            <li className={hasMix ? "ok" : ""}>{hasMix ? "✓" : "○"} 包含字母和数字</li>
          </ul>
        </>
      )}
    </div>
  );
}

export function passwordValid(v: string) {
  return v.length >= 8 && /[a-zA-Z]/.test(v) && /\d/.test(v);
}
const emailValid = (v: string) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v);

function CodeInput({
  disabled,
  error,
  onComplete,
}: {
  disabled?: boolean;
  error?: boolean;
  onComplete: (code: string) => void;
}) {
  const [digits, setDigits] = useState(["", "", "", "", "", ""]);
  const refs = useRef<(HTMLInputElement | null)[]>([]);

  function set(i: number, v: string) {
    if (v.length > 1) {
      const chars = v.replace(/\D/g, "").slice(0, 6).split("");
      const next = ["", "", "", "", "", ""];
      chars.forEach((c, j) => (next[j] = c));
      setDigits(next);
      if (chars.length === 6) onComplete(chars.join(""));
      else refs.current[chars.length]?.focus();
      return;
    }
    if (v && !/\d/.test(v)) return;
    const next = [...digits];
    next[i] = v;
    setDigits(next);
    if (v && i < 5) refs.current[i + 1]?.focus();
    if (next.every((d) => d !== "")) onComplete(next.join(""));
  }

  function key(i: number, e: React.KeyboardEvent) {
    if (e.key === "Backspace" && !digits[i] && i > 0) refs.current[i - 1]?.focus();
  }

  useEffect(() => {
    if (error) setDigits(["", "", "", "", "", ""]);
  }, [error]);

  return (
    <div className={`code-boxes ${error ? "error" : ""}`}>
      {digits.map((d, i) => (
        <input
          key={i}
          ref={(el) => (refs.current[i] = el)}
          value={d}
          inputMode="numeric"
          disabled={disabled}
          aria-label={`第 ${i + 1} 位，共 6 位`}
          onChange={(e) => set(i, e.target.value)}
          onKeyDown={(e) => key(i, e)}
        />
      ))}
    </div>
  );
}

function AuthPage({ children }: { children: React.ReactNode }) {
  return (
    <div className="auth-page">
      <div className="auth-card">{children}</div>
    </div>
  );
}

/* ---------- 注册 ---------- */

export function Signup() {
  const { showErrors, setEmail, invited } = useDemo();
  const nav = useNavigate();
  const [email, setLocalEmail] = useState("");
  const [emailErr, setEmailErr] = useState("");
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  const canSubmit = emailValid(email) && passwordValid(pw) && !busy;

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    await sleep(900);
    setBusy(false);
    if (showErrors) {
      setEmailErr("already");
      return;
    }
    setEmail(email);
    nav("/verify-email");
  }

  return (
    <AuthPage>
      <h2>创建账号</h2>
      <p className="sub">开始你的防封 Claude Code 工作区。</p>
      {invited && (
        <div className="banner ok">
          🎁 Alex 邀请你使用CC避风港 — 注册并登录 APP 即享首月免费试用。
        </div>
      )}
      <form onSubmit={submit}>
        <div className="field">
          <label>邮箱</label>
          <input
            type="email"
            value={email}
            placeholder="you@example.com"
            className={emailErr ? "invalid" : ""}
            onChange={(e) => { setLocalEmail(e.target.value); setEmailErr(""); }}
            onBlur={() => setEmailErr(email && !emailValid(email) ? "format" : "")}
          />
          {emailErr === "format" && <div className="err">请输入有效的邮箱地址。</div>}
          {emailErr === "already" && (
            <div className="err">
              该邮箱已注册。<Link to="/login">登录</Link> 或 <Link to="/forgot-password">找回密码</Link>。
            </div>
          )}
        </div>
        <PasswordField value={pw} onChange={setPw} />
        <button className="btn btn-primary" disabled={!canSubmit}>
          {busy && <span className="spinner" />}
          {busy ? "创建中…" : "创建账号"}
        </button>
      </form>
      <div className="auth-links">
        已有账号？<Link to="/login">登录</Link>
      </div>
      <div className="terms">继续即表示你同意服务条款及隐私政策。</div>
    </AuthPage>
  );
}

/* ---------- 邮箱验证 ---------- */

export function VerifyEmail() {
  const { email, showErrors, toast } = useDemo();
  const [attempts, setAttempts] = useState(5);
  const [errKey, setErrKey] = useState(0);
  const [errMsg, setErrMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [cooldown, setCooldown] = useState(10);

  useEffect(() => {
    if (cooldown <= 0) return;
    const t = setTimeout(() => setCooldown((c) => c - 1), 1000);
    return () => clearTimeout(t);
  }, [cooldown]);

  async function check(code: string) {
    setBusy(true);
    setErrMsg("");
    await sleep(700);
    setBusy(false);
    if (code === DEMO_CODE && !showErrors) {
      setDone(true);
      return;
    }
    const left = attempts - 1;
    setAttempts(left);
    setErrKey((k) => k + 1);
    setErrMsg(left > 0 ? `验证码不正确，还剩 ${left} 次尝试机会。` : "尝试次数过多，请重新发送验证码。");
  }

  if (done) {
    return (
      <AuthPage>
        <div className="success-check">✓</div>
        <h2>邮箱验证成功</h2>
        <p className="sub">一切就绪。下载应用并登录，即可开始使用。</p>
        <Link to="/download" className="btn btn-primary" style={{ display: "block", marginBottom: 10 }}>
          下载 macOS 版
        </Link>
        <Link to="/app" className="btn btn-secondary" style={{ display: "block" }}>
          打开CC避风港 APP
        </Link>
      </AuthPage>
    );
  }

  return (
    <AuthPage>
      <h2>请查收邮件</h2>
      <p className="sub">
        我们已向 <strong>{email}</strong> 发送 6 位验证码{" "}
        <Link to="/signup" style={{ fontSize: 13 }}>更改</Link>
      </p>
      <CodeInput disabled={busy || attempts <= 0} error={errKey > 0 && !!errMsg} onComplete={check} />
      {busy && <p style={{ fontSize: 13, color: "var(--gray)", marginTop: 8 }}><span className="spinner dark" /> 验证中…</p>}
      {errMsg && <p className="err" style={{ marginTop: 8, color: "var(--red)", fontSize: 13 }}>{errMsg}</p>}
      <div className="auth-links">
        {cooldown > 0 ? (
          <span>{cooldown} 秒后可重新发送</span>
        ) : (
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              setCooldown(10);
              setAttempts(5);
              setErrMsg("");
              toast("验证码已发送，请查收邮件。");
            }}
          >
            重新发送验证码
          </a>
        )}
      </div>
      <div className="terms">原型提示：验证码为 {DEMO_CODE}；冷却时间已压缩为 10 秒（规格为 60 秒）。</div>
    </AuthPage>
  );
}

/* ---------- 登录 ---------- */

export function Login() {
  const { showErrors, setAuthed, setEmail } = useDemo();
  const nav = useNavigate();
  const [email, setLocalEmail] = useState("");
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<"" | "creds" | "unverified">("");
  const canSubmit = emailValid(email) && pw.length > 0 && !busy;

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    setErr("");
    await sleep(800);
    setBusy(false);
    if (showErrors) {
      setErr("creds");
      setPw("");
      return;
    }
    setEmail(email);
    setAuthed(true);
    nav("/pricing");
  }

  return (
    <AuthPage>
      <h2>登录</h2>
      <p className="sub">欢迎回来。</p>
      {err === "unverified" && (
        <div className="banner warn">
          <span>你的邮箱尚未验证。</span>
          <button className="btn btn-secondary" onClick={() => nav("/verify-email")}>
            重新发送验证邮件
          </button>
        </div>
      )}
      <form onSubmit={submit}>
        <div className="field">
          <label>邮箱</label>
          <input type="email" value={email} placeholder="you@example.com" onChange={(e) => setLocalEmail(e.target.value)} />
        </div>
        <div className="field">
          <label style={{ display: "flex", justifyContent: "space-between" }}>
            密码
            <Link to="/forgot-password" style={{ fontWeight: 400, fontSize: 13 }}>
              忘记密码？
            </Link>
          </label>
          <input
            type="password"
            value={pw}
            placeholder="••••••••"
            className={err === "creds" ? "invalid" : ""}
            onChange={(e) => { setPw(e.target.value); setErr(""); }}
          />
          {err === "creds" && <div className="err">邮箱或密码不正确。</div>}
        </div>
        <button className="btn btn-primary" disabled={!canSubmit}>
          {busy && <span className="spinner" />}
          {busy ? "登录中…" : "登录"}
        </button>
      </form>
      <div className="auth-links">
        新用户？<Link to="/signup">创建账号</Link>
      </div>
    </AuthPage>
  );
}

/* ---------- 忘记密码 / 重设密码 ---------- */

export function ForgotPassword() {
  const [email, setLocalEmail] = useState("");
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!emailValid(email)) return;
    setBusy(true);
    await sleep(800);
    setBusy(false);
    setSent(true);
  }

  if (sent) {
    return (
      <AuthPage>
        <div className="success-check" style={{ background: "var(--blue)" }}>✉</div>
        <h2>请查收邮件</h2>
        <p className="sub">
          如 <strong>{email}</strong> 已注册账号，你将很快收到重设链接。
        </p>
        <Link to="/reset-password?token=demo" className="btn btn-secondary" style={{ display: "block", marginBottom: 10 }}>
          （原型演示：打开邮件里的链接）
        </Link>
        <div className="auth-links">
          <Link to="/login">返回登录</Link>
        </div>
      </AuthPage>
    );
  }

  return (
    <AuthPage>
      <h2>找回密码</h2>
      <p className="sub">输入你的账号邮箱，我们会发送重设链接给你。</p>
      <form onSubmit={submit}>
        <div className="field">
          <label>邮箱</label>
          <input type="email" value={email} placeholder="you@example.com" onChange={(e) => setLocalEmail(e.target.value)} />
        </div>
        <button className="btn btn-primary" disabled={!emailValid(email) || busy}>
          {busy && <span className="spinner" />}
          {busy ? "发送中…" : "发送重设链接"}
        </button>
      </form>
      <div className="auth-links">
        <Link to="/login">返回登录</Link>
      </div>
    </AuthPage>
  );
}

export function ResetPassword() {
  const { showErrors } = useDemo();
  const [params] = useSearchParams();
  const nav = useNavigate();
  const [checking, setChecking] = useState(true);
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);

  useEffect(() => {
    const t = setTimeout(() => setChecking(false), 800);
    return () => clearTimeout(t);
  }, []);

  const expired = showErrors || !params.get("token");

  if (checking) {
    return (
      <AuthPage>
        <div className="skeleton" style={{ height: 22, width: 200, margin: "8px auto 14px" }} />
        <div className="skeleton" style={{ height: 44, margin: "10px 0" }} />
        <div className="skeleton" style={{ height: 44, margin: "10px 0" }} />
      </AuthPage>
    );
  }

  if (expired) {
    return (
      <AuthPage>
        <h2>链接已失效</h2>
        <p className="sub">该链接已过期或已被使用。</p>
        <Link to="/forgot-password" className="btn btn-primary" style={{ display: "block" }}>
          重新申请链接
        </Link>
      </AuthPage>
    );
  }

  if (done) {
    return (
      <AuthPage>
        <div className="success-check">✓</div>
        <h2>密码已更新</h2>
        <p className="sub">所有设备已退出登录，正在跳转到登录页…</p>
      </AuthPage>
    );
  }

  const valid = passwordValid(pw) && pw === pw2;

  return (
    <AuthPage>
      <h2>设置新密码</h2>
      <p className="sub">修改后，你原有的登录会全部退出。</p>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          await sleep(900);
          setDone(true);
          setTimeout(() => nav("/login"), 2500);
        }}
      >
        <PasswordField label="新密码" value={pw} onChange={setPw} />
        <div className="field">
          <label>确认新密码</label>
          <input
            type="password"
            value={pw2}
            className={pw2 && pw2 !== pw ? "invalid" : ""}
            onChange={(e) => setPw2(e.target.value)}
          />
          {pw2 && pw2 !== pw && <div className="err">两次输入的密码不一致。</div>}
        </div>
        <button className="btn btn-primary" disabled={!valid || busy}>
          {busy && <span className="spinner" />}
          {busy ? "更新中…" : "更新密码"}
        </button>
      </form>
    </AuthPage>
  );
}
