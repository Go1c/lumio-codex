import { useEffect, useSyncExternalStore } from "react";

import { LumioCommandError } from "../../invoke.ts";
import {
  ensureClaudeEngineBridge,
  hydrateClaudeWorkspace,
  payClaudeSubscribe,
} from "../../claude/session.ts";
import { dispatchClaude, getClaudeState, subscribeClaudeStore } from "../../claude/store.ts";
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
  onOpenOrders: () => void;
  onRecharge: () => void;
  onPaid?: () => void;
  pushToast: (input: string) => void;
};

export function ClaudeWorkspace({
  account,
  onBackToCodex,
  onOpenOrders,
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
    <div className="lumio-claude-orders">
      <button className="lumio-link-button" onClick={onOpenOrders} type="button">
        开通记录
      </button>
    </div>
  );

  return (
    <div className="lumio-claude">
      {state.page === "subscribe" ? (
        <ClaudeSubscribe
          balance={account?.balance ?? 0}
          entitlement={state.entitlement}
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
          ordersSlot={ordersSlot}
          paying={state.paying}
          payMode={state.payMode}
          planAmountCents={state.planAmountCents}
        />
      ) : state.projects.length > 0 ? (
        <ClaudeHome onBackToCodex={onBackToCodex} onConnect={openConnect} state={state} />
      ) : (
        <ClaudeEmpty
          entitlement={state.entitlement}
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
