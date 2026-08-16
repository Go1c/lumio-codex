import { useEffect, useSyncExternalStore } from "react";

import { ensureClaudeEngineBridge, hydrateClaudeWorkspace } from "../../claude/session.ts";
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
  onOpenAccount: () => void;
};

export function ClaudeWorkspace({
  account,
  onBackToCodex,
  onOpenAccount,
}: ClaudeWorkspaceProps) {
  const state = useSyncExternalStore(subscribeClaudeStore, getClaudeState, getClaudeState);

  useEffect(() => {
    ensureClaudeEngineBridge();
    void hydrateClaudeWorkspace(account);
  }, [account?.email, account?.planLabel]);

  const openConnect = () => dispatchClaude({ type: "open-connect" });

  return (
    <div className="lumio-claude">
      {state.page === "subscribe" ? (
        <ClaudeSubscribe onBackToCodex={onBackToCodex} onOpenAccount={onOpenAccount} />
      ) : state.projects.length > 0 ? (
        <ClaudeHome onConnect={openConnect} state={state} />
      ) : (
        <ClaudeEmpty ghost={state.sheet !== null} onBackToCodex={onBackToCodex} onConnect={openConnect} />
      )}
      {state.sheet ? <ClaudeConnect onBackToCodex={onBackToCodex} sheet={state.sheet} /> : null}
    </div>
  );
}
