import type { ReactNode } from "react";

import { ClaudeEntitlementLine } from "./ClaudeEntitlementLine.tsx";
import type { ClaudeEntitlement } from "../../claude/types.ts";

const PROMISES = [
  { title: "独立环境", detail: "Claude 不跑在你的笔记本上，封号风险隔开。" },
  { title: "本机仍能改文件", detail: "远端改动回来，本地编辑上去。冲突不会被静默覆盖。" },
  { title: "一次登录", detail: "用的就是现在这个 BestCodex 账号，不用再注册。" },
] as const;

export function ClaudeEmpty({
  ghost,
  onConnect,
  onBackToCodex,
  ordersSlot,
  entitlement,
}: {
  ghost?: boolean;
  onConnect: () => void;
  onBackToCodex: () => void;
  ordersSlot?: ReactNode;
  entitlement?: ClaudeEntitlement;
}) {
  return (
    <main className={`lumio-claude-onboard${ghost ? " is-ghost" : ""}`} aria-hidden={ghost ? true : undefined}>
      <div className="lumio-claude-card">
        <span className="lumio-claude-icon" aria-hidden="true">
          <img alt="" src="/lumio-icon.png" />
        </span>
        <span className="lumio-claude-chip">Claude</span>
        <h2>
          把 Claude
          <br />
          放到你自己的服务器上。
        </h2>
        <p>固定 IP、持久会话、和本机双向同步。Codex Tab 不受影响，随时切回去。</p>
        {entitlement ? <ClaudeEntitlementLine entitlement={entitlement} /> : null}
        <div className="lumio-claude-promise">
          {PROMISES.map((item) => (
            <div key={item.title}>
              <b>{item.title}</b>
              <span>{item.detail}</span>
            </div>
          ))}
        </div>
        {ghost ? null : (
          <>
            <button className="lumio-button is-primary is-large" onClick={onConnect} type="button">
              连接一台服务器
            </button>
            {ordersSlot}
            <p className="lumio-claude-quiet">
              <button className="lumio-link-button" onClick={onBackToCodex} type="button">
                先留在 Codex
              </button>
            </p>
          </>
        )}
      </div>
    </main>
  );
}
