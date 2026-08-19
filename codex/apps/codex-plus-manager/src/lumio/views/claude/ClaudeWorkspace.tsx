import { useEffect, useSyncExternalStore } from "react";

import { LumioCommandError } from "../../invoke.ts";
import {
  ensureClaudeEngineBridge,
  formatClaudeOrderYuan,
  hydrateClaudeWorkspace,
  payClaudeSubscribe,
  toggleClaudeOrders,
} from "../../claude/session.ts";
import { dispatchClaude, getClaudeState, subscribeClaudeStore } from "../../claude/store.ts";
import type { ClaudeOrder } from "../../claude/types.ts";
import type { LumioAccountSummary } from "../../types.ts";
import { ClaudeConnect } from "./ClaudeConnect.tsx";
import { ClaudeEmpty } from "./ClaudeEmpty.tsx";
import { ClaudeHome } from "./ClaudeHome.tsx";
import { ClaudeSubscribe } from "./ClaudeSubscribe.tsx";
import "./claude-workspace.css";

export type ClaudeWorkspaceProps = {
  account: LumioAccountSummary | null;
  onBackToCodex: () => void;
  onOpenHelp: () => void;
  onRecharge: () => void;
  onPaid?: () => void;
  pushToast: (input: string) => void;
};

function orderStatusLabel(status: string): string {
  if (status === "paid") return "已支付";
  if (status === "pending") return "处理中";
  if (status === "failed") return "失败";
  return status;
}

function ClaudeOrderHistory({
  orders,
  open,
  onToggle,
}: {
  orders: ClaudeOrder[];
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="lumio-claude-orders">
      <button className="lumio-link-button" onClick={onToggle} type="button">
        开通记录
      </button>
      {open ? (
        orders.length === 0 ? (
          <p className="lumio-claude-quiet">暂无开通记录</p>
        ) : (
          <ul>
            {orders.map((order) => (
              <li key={order.orderNo}>
                <span>¥{formatClaudeOrderYuan(order.amountCents)}</span>
                <span>{orderStatusLabel(order.status)}</span>
                <span>{order.createdAt.slice(0, 10)}</span>
              </li>
            ))}
          </ul>
        )
      ) : null}
    </div>
  );
}

export function ClaudeWorkspace({
  account,
  onBackToCodex,
  onRecharge,
  onPaid,
  pushToast,
}: ClaudeWorkspaceProps) {
  const state = useSyncExternalStore(subscribeClaudeStore, getClaudeState, getClaudeState);

  useEffect(() => {
    ensureClaudeEngineBridge();
    void hydrateClaudeWorkspace(account);
  }, [account?.email, account?.planLabel]);

  useEffect(() => {
    if (getClaudeState().payMode === "recharge") {
      dispatchClaude({ type: "pay-finished" });
    }
  }, [account?.balance]);

  const openConnect = () => dispatchClaude({ type: "open-connect" });
  const ordersSlot = (
    <ClaudeOrderHistory
      open={state.ordersOpen}
      orders={state.orders}
      onToggle={() => toggleClaudeOrders()}
    />
  );

  return (
    <div className="lumio-claude">
      {state.page === "subscribe" ? (
        <ClaudeSubscribe
          balance={account?.balance ?? 0}
          onBackToCodex={onBackToCodex}
          onPay={() => {
            void payClaudeSubscribe(account)
              .then(() => onPaid?.())
              .catch((error: unknown) => {
                const code = error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
                pushToast(code);
              });
          }}
          onRecharge={onRecharge}
          paying={state.paying}
          payMode={state.payMode}
          planAmountCents={state.planAmountCents}
        />
      ) : state.projects.length > 0 ? (
        <ClaudeHome onConnect={openConnect} ordersSlot={ordersSlot} state={state} />
      ) : (
        <ClaudeEmpty
          ghost={state.sheet !== null}
          onBackToCodex={onBackToCodex}
          onConnect={openConnect}
          ordersSlot={ordersSlot}
        />
      )}
      {state.sheet ? <ClaudeConnect onBackToCodex={onBackToCodex} sheet={state.sheet} /> : null}
    </div>
  );
}
