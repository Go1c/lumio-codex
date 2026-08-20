import { useId, useState, type FormEvent } from "react";

export type LoginCardLayout = "embedded" | "overlay";

export type LoginCardProps = {
  layout?: LoginCardLayout;
  loginUrl: string | null;
  claudeVersion?: string;
  onOpenBrowser: () => void;
  onCopyLink: () => void;
  onSubmitCode: (code: string) => void;
};

export function LoginCard({
  layout = "embedded",
  loginUrl,
  claudeVersion,
  onOpenBrowser,
  onCopyLink,
  onSubmitCode,
}: LoginCardProps) {
  const [code, setCode] = useState("");
  const fieldId = useId();
  const overlay = layout === "overlay";
  const trimmed = code.trim();

  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    if (!trimmed) return;
    onSubmitCode(trimmed);
  };

  const body = (
    <>
      {overlay ? (
        <header>
          <h3>登录已过期</h3>
          <span className="lumio-claude-init-pill is-warn">重新授权一次</span>
        </header>
      ) : null}
      {overlay ? (
        <p className="lumio-claude-init-meta">
          服务器和文件都是好的，Claude 也还是 {claudeVersion ?? "2.1.228"}。只是授权到期了。
        </p>
      ) : null}
      <div className="lumio-claude-actions">
        <button className="lumio-button is-primary" onClick={onOpenBrowser} type="button">
          在浏览器中登录
        </button>
        <button className="lumio-button is-secondary" disabled={!loginUrl} onClick={onCopyLink} type="button">
          复制登录链接
        </button>
      </div>
      <label className="lumio-claude-paste-label" htmlFor={fieldId}>
        浏览器给了授权码？贴在这里
      </label>
      <form className="lumio-claude-paste-row" onSubmit={onSubmit}>
        <input
          className="lumio-claude-field"
          id={fieldId}
          onChange={(event) => setCode(event.target.value)}
          placeholder="粘贴授权码"
          value={code}
        />
        <button className="lumio-button is-primary lumio-claude-login-submit" disabled={!trimmed} type="submit">
          完成登录
        </button>
      </form>
      <p className="lumio-claude-init-quiet">
        {overlay
          ? "后面那段对话还在，授权完接着用。"
          : "不用在黑底窗口里复制几十行地址、再对着提示盲贴。"}
      </p>
    </>
  );

  if (overlay) {
    return (
      <div className="lumio-claude-login is-overlay" role="dialog" aria-label="登录已过期">
        <div className="lumio-claude-ws-card lumio-claude-login-card">{body}</div>
      </div>
    );
  }

  return <div className="lumio-claude-login">{body}</div>;
}
