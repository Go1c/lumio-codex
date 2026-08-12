import { Link } from "react-router-dom";

export function NotFound() {
  return (
    <div className="dl-hero">
      <h1 style={{ fontSize: 32 }}>页面不存在</h1>
      <p className="section-sub">链接可能已过期，或地址输错了。</p>
      <Link to="/" className="btn btn-primary">
        回到首页
      </Link>
    </div>
  );
}
