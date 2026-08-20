import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";

import { fetchClaudeServerStatus } from "../../claude/session.ts";
import { dispatchClaude, getClaudeState, subscribeClaudeStore } from "../../claude/store.ts";
import { workspaceStatusCopy } from "../../claude/sync-status.ts";
import type {
  ClaudeProject,
  ClaudeServerStatus,
  ClaudeStatusDrawerPane,
  ClaudeSyncStatus,
} from "../../claude/types.ts";
import {
  claudeVersionLoginCopy,
  collectSessions,
  conflictFlagCopy,
  conversationCountCopy,
  hostResourceCopy,
  readyStatusCopy,
  updateNudgeCopy,
} from "./status-copy.ts";

const STATUS_POLL_MS = 30_000;

export function StatusBar({
  sync,
  active,
}: {
  sync: ClaudeSyncStatus | null;
  active: ClaudeProject | null;
}) {
  const state = useSyncExternalStore(subscribeClaudeStore, getClaudeState, getClaudeState);
  const [snapshot, setSnapshot] = useState<ClaudeServerStatus | null>(null);

  useEffect(() => {
    if (!active) {
      setSnapshot(null);
      return;
    }
    let cancelled = false;
    const load = () => {
      void fetchClaudeServerStatus(active.id).then((next) => {
        if (!cancelled) setSnapshot(next);
      });
    };
    load();
    const timer = window.setInterval(load, STATUS_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [active?.id]);

  const phase = active ? state.workspacePhaseByProject[active.id] : undefined;
  const cli = active ? state.cliByHost[active.host] : null;
  const login = active ? state.loginByHost[active.host] : null;
  const ready = readyStatusCopy(phase, sync);
  const claudeLine = active ? claudeVersionLoginCopy(cli, login) : null;
  const resources = snapshot?.ok ? hostResourceCopy(snapshot.host) : null;
  const conflictCount = active
    ? (state.conflictsByProject[active.id]?.length || sync?.conflicts || 0)
    : 0;
  const flag = conflictFlagCopy(conflictCount);
  const nudge = updateNudgeCopy(cli);
  const sessions = collectSessions(state.sessionsByProject);
  const running = sessions.filter((session) => session.running).length;
  const conversation = conversationCountCopy(sessions.length, running);
  const syncPane: ClaudeStatusDrawerPane = conflictCount > 0 ? "conflicts" : "server";

  return (
    <footer className="lumio-claude-status">
      {active ? (
        <>
          <StatusSeg pane="server">
            <span className={`is-${ready.tone}`}>● {ready.label}</span>
          </StatusSeg>
          <span className="lumio-claude-status-sep">·</span>
          <StatusSeg pane="server">{claudeLine}</StatusSeg>
          <span className="lumio-claude-status-sep">·</span>
          <StatusSeg className={sync?.state === "conflicts" ? "is-warn" : undefined} pane={syncPane}>
            {workspaceStatusCopy(sync)}
          </StatusSeg>
          {resources ? (
            <>
              <span className="lumio-claude-status-sep">·</span>
              <StatusSeg pane="server">{resources}</StatusSeg>
            </>
          ) : null}
        </>
      ) : null}
      <span className="lumio-claude-status-grow" />
      {nudge ? <span className="lumio-claude-status-nudge">{nudge}</span> : null}
      {flag ? (
        <StatusSeg className="lumio-claude-status-flag" pane="conflicts">
          {flag}
        </StatusSeg>
      ) : null}
      <StatusSeg pane="sessions">
        {running > 0 ? `对话 ${sessions.length} · ${running} 在跑` : conversation}
      </StatusSeg>
      <span className="lumio-claude-status-host">{active ? `${active.user}@${active.host}` : ""}</span>
    </footer>
  );
}

function StatusSeg({
  pane,
  children,
  className,
}: {
  pane: ClaudeStatusDrawerPane;
  children: ReactNode;
  className?: string;
}) {
  return (
    <button
      className={className}
      onClick={() => dispatchClaude({ type: "set-status-drawer", pane })}
      type="button"
    >
      {children}
    </button>
  );
}
