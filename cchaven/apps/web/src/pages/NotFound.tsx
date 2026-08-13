import { Link } from "react-router-dom";

export function NotFound() {
  return (
    <div className="auth-page">
      <div className="auth-card">
        <h2>页面不存在</h2>
        <p className="sub">这个地址已经失效，或者你输错了链接。</p>
        <Link to="/" className="btn btn-primary btn-block">
          返回首页
        </Link>
      </div>
    </div>
  );
}
