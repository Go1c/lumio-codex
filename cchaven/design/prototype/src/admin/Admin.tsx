import { useState } from "react";
import { sleep, useDemo } from "../demo";

type Page = "dash" | "users" | "orders" | "settings";

const NAV: [Page, string, string][] = [
  ["dash", "📊", "仪表盘"],
  ["users", "👥", "用户"],
  ["orders", "🧾", "订单与付款"],
  ["settings", "⚙️", "运营配置"],
];

export default function AdminShell() {
  const [page, setPage] = useState<Page>("dash");
  return (
    <div className="appframe">
      <aside className="app-sidebar" style={{ width: 210 }}>
        <div style={{ padding: "10px 10px 4px", color: "#fff", fontSize: 14.5, fontWeight: 700 }}>
          CC避风港 <span style={{ fontWeight: 400, color: "#8b8f98", fontSize: 12 }}>运营后台</span>
        </div>
        <div style={{ padding: "0 10px 14px", fontSize: 11.5, color: "#8b8f98" }}>内部系统 · 仅限管理员</div>
        {NAV.map(([k, ic, label]) => (
          <div key={k} className={`proj-item ${page === k ? "active" : ""}`} onClick={() => setPage(k)}>
            <span>{ic}</span>
            {label}
          </div>
        ))}
        <div className="sidebar-bottom">
          <div className="account-chip">
            <span className="avatar" style={{ background: "#374151" }}>A</span>
            <div style={{ minWidth: 0, lineHeight: 1.3 }}>
              <div style={{ overflow: "hidden", textOverflow: "ellipsis" }}>admin@cchaven.cn</div>
              <div style={{ fontSize: 11, color: "#8b8f98" }}>超级管理员</div>
            </div>
          </div>
        </div>
      </aside>
      <main className="app-main" style={{ background: "var(--bg)", overflow: "auto" }}>
        {page === "dash" && <Dashboard />}
        {page === "users" && <Users />}
        {page === "orders" && <Orders />}
        {page === "settings" && <Settings />}
      </main>
    </div>
  );
}

/* ---------- 仪表盘 ---------- */

const DAU_7D = [
  { d: "8/6", v: 980 },
  { d: "8/7", v: 1010 },
  { d: "8/8", v: 1052 },
  { d: "8/9", v: 1121 },
  { d: "8/10", v: 1087 },
  { d: "8/11", v: 1214 },
  { d: "今天", v: 1284 },
];

const STATS = [
  { label: "今日日活（DAU）", value: "1,284", sub: "较昨日 +5.8%", up: true },
  { label: "今日新增注册", value: "96", sub: "其中经邀请 41 人", up: true },
  { label: "付费订阅用户", value: "862", sub: "试用中 214 人", up: true },
  { label: "今日收入", value: "¥3,264", sub: "48 笔订单", up: true },
  { label: "试用 → 付费转化率", value: "38.2%", sub: "近 30 天", up: true },
  { label: "7 日留存", value: "61.4%", sub: "较上周 -1.2%", up: false },
];

function Dashboard() {
  const max = Math.max(...DAU_7D.map((x) => x.v));
  return (
    <div className="adm-page">
      <h2>仪表盘</h2>
      <div className="stat-grid">
        {STATS.map((s) => (
          <div className="stat-card" key={s.label}>
            <div className="lb">{s.label}</div>
            <div className="v">{s.value}</div>
            <div className="sub" style={{ color: s.up ? "var(--green)" : "var(--orange)" }}>{s.sub}</div>
          </div>
        ))}
      </div>

      <div className="adm-cols">
        <section className="adm-card">
          <h3>近 7 日日活</h3>
          <div className="bar-chart">
            {DAU_7D.map((x) => (
              <div className="bar-col" key={x.d}>
                <div className="bar-v">{x.v}</div>
                <div className="bar" style={{ height: `${(x.v / max) * 100}%` }} />
                <div className="bar-d">{x.d}</div>
              </div>
            ))}
          </div>
        </section>

        <section className="adm-card">
          <h3>使用平台分布（近 30 天活跃）</h3>
          {(
            [
              ["macOS · Apple Silicon", 78],
              ["macOS · Intel", 22],
            ] as [string, number][]
          ).map(([label, pct]) => (
            <div className="dist-row" key={label}>
              <span className="dl">{label}</span>
              <div className="dist-track"><div className="dist-fill" style={{ width: `${pct}%` }} /></div>
              <span className="dp">{pct}%</span>
            </div>
          ))}
          <h3 style={{ marginTop: 22 }}>APP 版本分布</h3>
          {(
            [
              ["1.4.2（最新）", 64],
              ["1.4.1", 27],
              ["≤ 1.4.0", 9],
            ] as [string, number][]
          ).map(([label, pct]) => (
            <div className="dist-row" key={label}>
              <span className="dl">{label}</span>
              <div className="dist-track"><div className="dist-fill" style={{ width: `${pct}%`, background: "#7c3aed" }} /></div>
              <span className="dp">{pct}%</span>
            </div>
          ))}
          <h3 style={{ marginTop: 22 }}>注册来源（近 30 天）</h3>
          {(
            [
              ["自然流量", 52],
              ["好友邀请", 38],
              ["其他渠道", 10],
            ] as [string, number][]
          ).map(([label, pct]) => (
            <div className="dist-row" key={label}>
              <span className="dl">{label}</span>
              <div className="dist-track"><div className="dist-fill" style={{ width: `${pct}%`, background: "var(--green)" }} /></div>
              <span className="dp">{pct}%</span>
            </div>
          ))}
        </section>
      </div>
    </div>
  );
}

/* ---------- 用户 ---------- */

type SubState = "sub" | "trial" | "none" | "banned";
const SUB_TAG: Record<SubState, { label: string; cls: string }> = {
  sub: { label: "已订阅", cls: "t-blue" },
  trial: { label: "试用中", cls: "t-green" },
  none: { label: "未订阅", cls: "t-gray" },
  banned: { label: "已禁用", cls: "t-red" },
};

interface AdmUser {
  id: string;
  email: string;
  regAt: string;
  source: string;
  platform: string;
  sub: SubState;
  lastActive: string;
}

const INITIAL_USERS: AdmUser[] = [
  { id: "U-100986", email: "mary@example.com", regAt: "2026-06-02", source: "自然流量", platform: "macOS 15 · AS", sub: "sub", lastActive: "刚刚" },
  { id: "U-100985", email: "wang***@gmail.com", regAt: "2026-08-11", source: "邀请（U-100986）", platform: "macOS 14 · AS", sub: "trial", lastActive: "13 分钟前" },
  { id: "U-100984", email: "li***3@qq.com", regAt: "2026-08-11", source: "邀请（U-100986）", platform: "—（未登录 APP）", sub: "none", lastActive: "—" },
  { id: "U-100983", email: "chen***@163.com", regAt: "2026-08-10", source: "自然流量", platform: "macOS 15 · Intel", sub: "sub", lastActive: "2 小时前" },
  { id: "U-100982", email: "zhao***@outlook.com", regAt: "2026-08-09", source: "其他渠道", platform: "macOS 14 · AS", sub: "trial", lastActive: "昨天" },
  { id: "U-100981", email: "sun***@qq.com", regAt: "2026-08-08", source: "自然流量", platform: "macOS 13 · Intel", sub: "none", lastActive: "3 天前" },
  { id: "U-100980", email: "spam***@tmp.io", regAt: "2026-08-07", source: "其他渠道", platform: "macOS 14 · AS", sub: "banned", lastActive: "5 天前" },
];

function Users() {
  const { showErrors, toast } = useDemo();
  const [users, setUsers] = useState(INITIAL_USERS);
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<SubState | "all">("all");

  if (showErrors) {
    return (
      <div className="adm-page">
        <h2>用户</h2>
        <div className="banner error"><span>用户数据加载失败：服务暂时不可用。</span><button className="btn btn-secondary">重试</button></div>
      </div>
    );
  }

  const list = users.filter(
    (u) =>
      (filter === "all" || u.sub === filter) &&
      (q === "" || u.email.includes(q) || u.id.toLowerCase().includes(q.toLowerCase())),
  );

  function toggleBan(u: AdmUser) {
    const banning = u.sub !== "banned";
    if (banning && !confirm(`确定禁用 ${u.id}（${u.email}）吗？\n\n该用户将立即被登出且无法登录。`)) return;
    setUsers(users.map((x) => (x.id === u.id ? { ...x, sub: banning ? "banned" : "none" } : x)));
    toast(banning ? `已禁用 ${u.id}。` : `已解禁 ${u.id}。`);
  }

  return (
    <div className="adm-page">
      <h2>用户 <span className="adm-count">{users.length.toLocaleString()} 人（演示数据）</span></h2>
      <div className="adm-toolbar">
        <input className="adm-search" placeholder="搜索邮箱 / 用户 ID…" value={q} onChange={(e) => setQ(e.target.value)} />
        {(["all", "sub", "trial", "none", "banned"] as const).map((f) => (
          <button key={f} className={`chip ${filter === f ? "on" : ""}`} onClick={() => setFilter(f)}>
            {f === "all" ? "全部" : SUB_TAG[f].label}
          </button>
        ))}
      </div>
      <div className="adm-card" style={{ padding: 0 }}>
        <table className="adm-table">
          <thead>
            <tr>
              <th>用户 ID</th><th>邮箱</th><th>注册时间</th><th>来源</th><th>使用平台</th><th>订阅状态</th><th>最近活跃</th><th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((u) => (
              <tr key={u.id}>
                <td style={{ fontFamily: "Menlo, monospace", fontSize: 12.5 }}>{u.id}</td>
                <td>{u.email}</td>
                <td>{u.regAt}</td>
                <td>{u.source}</td>
                <td>{u.platform}</td>
                <td><span className={`tag ${SUB_TAG[u.sub].cls}`}>{SUB_TAG[u.sub].label}</span></td>
                <td>{u.lastActive}</td>
                <td>
                  <button className="btn btn-ghost" style={{ padding: "2px 8px", fontSize: 12, color: u.sub === "banned" ? "var(--green)" : "var(--red)" }} onClick={() => toggleBan(u)}>
                    {u.sub === "banned" ? "解禁" : "禁用"}
                  </button>
                </td>
              </tr>
            ))}
            {list.length === 0 && (
              <tr><td colSpan={8} style={{ textAlign: "center", color: "var(--gray)", padding: 28 }}>没有匹配的用户。</td></tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/* ---------- 订单与付款 ---------- */

type OrderState = "paid" | "refunding" | "refunded" | "failed";
const ORDER_TAG: Record<OrderState, { label: string; cls: string }> = {
  paid: { label: "已支付", cls: "t-green" },
  refunding: { label: "退款中", cls: "t-orange" },
  refunded: { label: "已退款", cls: "t-gray" },
  failed: { label: "支付失败", cls: "t-red" },
};

interface Order {
  no: string;
  user: string;
  amount: string;
  channel: string;
  state: OrderState;
  at: string;
}

const INITIAL_ORDERS: Order[] = [
  { no: "CC20260812-100486", user: "mary@example.com", amount: "¥68.00", channel: "支付宝", state: "paid", at: "2026-08-12 09:41" },
  { no: "CC20260812-100485", user: "chen***@163.com", amount: "¥68.00", channel: "微信支付", state: "paid", at: "2026-08-12 08:17" },
  { no: "CC20260811-100471", user: "zhao***@outlook.com", amount: "¥68.00", channel: "支付宝", state: "failed", at: "2026-08-11 22:03" },
  { no: "CC20260811-100468", user: "sun***@qq.com", amount: "¥68.00", channel: "银行卡", state: "refunded", at: "2026-08-11 16:44" },
  { no: "CC20260811-100455", user: "wang***@gmail.com", amount: "¥68.00", channel: "微信支付", state: "paid", at: "2026-08-11 10:29" },
  { no: "CC20260810-100440", user: "chen***@163.com", amount: "¥68.00", channel: "支付宝", state: "paid", at: "2026-08-10 19:12" },
];

function Orders() {
  const { showErrors, toast } = useDemo();
  const [orders, setOrders] = useState(INITIAL_ORDERS);
  const [filter, setFilter] = useState<OrderState | "all">("all");

  if (showErrors) {
    return (
      <div className="adm-page">
        <h2>订单与付款</h2>
        <div className="banner error"><span>订单数据加载失败：支付服务商接口超时。</span><button className="btn btn-secondary">重试</button></div>
      </div>
    );
  }

  const list = orders.filter((o) => filter === "all" || o.state === filter);

  async function refund(o: Order) {
    if (!confirm(`确定对订单 ${o.no}（${o.amount}）发起退款吗？`)) return;
    setOrders(orders.map((x) => (x.no === o.no ? { ...x, state: "refunding" } : x)));
    toast(`退款已发起，订单 ${o.no} 进入退款中。`);
    await sleep(2500);
    setOrders((cur) => cur.map((x) => (x.no === o.no ? { ...x, state: "refunded" } : x)));
    toast(`订单 ${o.no} 退款完成。`);
  }

  return (
    <div className="adm-page">
      <h2>订单与付款 <span className="adm-count">今日 48 笔 · ¥3,264（演示数据）</span></h2>
      <div className="adm-toolbar">
        {(["all", "paid", "refunding", "refunded", "failed"] as const).map((f) => (
          <button key={f} className={`chip ${filter === f ? "on" : ""}`} onClick={() => setFilter(f)}>
            {f === "all" ? "全部" : ORDER_TAG[f].label}
          </button>
        ))}
        <span style={{ flex: 1 }} />
        <button className="btn btn-secondary" style={{ padding: "6px 14px", fontSize: 13 }} onClick={() => toast("原型：导出 CSV（按当前筛选）。")}>
          导出 CSV
        </button>
      </div>
      <div className="adm-card" style={{ padding: 0 }}>
        <table className="adm-table">
          <thead>
            <tr><th>订单号</th><th>用户</th><th>金额</th><th>支付渠道</th><th>状态</th><th>时间</th><th></th></tr>
          </thead>
          <tbody>
            {list.map((o) => (
              <tr key={o.no}>
                <td style={{ fontFamily: "Menlo, monospace", fontSize: 12.5 }}>
                  {o.no}
                  <button className="btn btn-ghost" style={{ padding: "0 6px", fontSize: 11 }} title="复制订单号" onClick={() => toast("已复制订单号。")}>⧉</button>
                </td>
                <td>{o.user}</td>
                <td>{o.amount}</td>
                <td>{o.channel}</td>
                <td><span className={`tag ${ORDER_TAG[o.state].cls}`}>{ORDER_TAG[o.state].label}</span></td>
                <td>{o.at}</td>
                <td>
                  {o.state === "paid" && (
                    <button className="btn btn-ghost" style={{ padding: "2px 8px", fontSize: 12, color: "var(--red)" }} onClick={() => refund(o)}>
                      退款
                    </button>
                  )}
                </td>
              </tr>
            ))}
            {list.length === 0 && (
              <tr><td colSpan={7} style={{ textAlign: "center", color: "var(--gray)", padding: 28 }}>该状态下没有订单。</td></tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/* ---------- 运营配置 ---------- */

function Settings() {
  const { toast } = useDemo();
  const [rewardDays, setRewardDays] = useState("7");
  const [trialDays, setTrialDays] = useState("30");
  const [price, setPrice] = useState("68");
  const [saving, setSaving] = useState(false);

  async function save() {
    setSaving(true);
    await sleep(700);
    setSaving(false);
    toast("配置已保存，官网与 APP 端实时生效。");
  }

  return (
    <div className="adm-page" style={{ maxWidth: 620 }}>
      <h2>运营配置</h2>
      <section className="adm-card">
        <h3>邀请裂变</h3>
        <div className="field">
          <label>邀请者奖励：每成功邀请 1 人，订阅延长（天）</label>
          <input value={rewardDays} onChange={(e) => setRewardDays(e.target.value)} style={{ width: 120 }} />
          <div className="hint">配为 0 即关闭邀请者奖励，前端相关文案自动隐藏。</div>
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>被邀请者免费试用时长（天）</label>
          <input value={trialDays} onChange={(e) => setTrialDays(e.target.value)} style={{ width: 120 }} />
          <div className="hint">每个账号一生仅可享用一次，由后端设备/支付指纹防重复领取。</div>
        </div>
      </section>
      <section className="adm-card">
        <h3>定价</h3>
        <div className="field" style={{ marginBottom: 0 }}>
          <label>包月价格（¥ / 月）</label>
          <input value={price} onChange={(e) => setPrice(e.target.value)} style={{ width: 120 }} />
          <div className="hint">官网定价页与账户中心从此配置读取，页面不写死价格。</div>
        </div>
      </section>
      <button className="btn btn-primary" onClick={save} disabled={saving}>
        {saving && <span className="spinner" />}
        {saving ? "保存中…" : "保存配置"}
      </button>
    </div>
  );
}
