import { Link, Outlet } from "react-router-dom";
import { useDemo } from "../demo";

export default function SiteLayout() {
  const { authed, email } = useDemo();
  return (
    <div className="site">
      <header className="site-header">
        <Link to="/" className="logo">
          <span className="mark" /> CC避风港 <span style={{ fontWeight: 400, color: "var(--gray)", fontSize: 13 }}>CCHaven</span>
        </Link>
        <nav className="site-nav">
          <Link to="/pricing">定价</Link>
          <a href="#" onClick={(e) => e.preventDefault()}>文档</a>
          <Link to="/download">下载</Link>
        </nav>
        <span className="spacer" />
        <nav className="site-nav" style={{ alignItems: "center", gap: 16 }}>
          {authed ? (
            <Link to="/account" style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <span className="avatar" style={{ width: 24, height: 24, lineHeight: "24px", fontSize: 11 }}>
                {email[0]?.toUpperCase() ?? "U"}
              </span>
              账户
            </Link>
          ) : (
            <>
              <Link to="/login">登录</Link>
              <Link to="/signup" className="btn btn-primary" style={{ padding: "8px 16px" }}>
                免费开始
              </Link>
            </>
          )}
        </nav>
      </header>
      <Outlet />
      <footer className="site-footer">
        <span>© 2026 CC避风港 CCHaven</span>
        <a href="#" onClick={(e) => e.preventDefault()}>服务条款</a>
        <a href="#" onClick={(e) => e.preventDefault()}>隐私政策</a>
        <a href="#" onClick={(e) => e.preventDefault()}>系统状态</a>
      </footer>
    </div>
  );
}
