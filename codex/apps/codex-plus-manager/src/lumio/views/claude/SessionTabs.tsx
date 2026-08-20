import type { JSX, MouseEvent } from "react";

import { clipWidth, SESSION_TITLE_WIDTH } from "../../claude/session-title.ts";
import type { ClaudeChatSession } from "../../claude/types.ts";

export function SessionTabs({
  sessions,
  activeSessionId,
  onSelect,
  onNew,
  onClose,
  onAskClose,
  askingId,
  onConfirmClose,
  onCancelClose,
}: {
  sessions: ClaudeChatSession[];
  activeSessionId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onClose: (id: string) => void;
  onAskClose: (id: string) => void;
  askingId: string | null;
  onConfirmClose: () => void;
  onCancelClose: () => void;
}): JSX.Element {
  const asking = askingId != null && sessions.some((session) => session.id === askingId);

  const select = (id: string) => {
    if (askingId) onCancelClose();
    onSelect(id);
  };

  const create = () => {
    if (askingId) onCancelClose();
    onNew();
  };

  const closeTab = (event: MouseEvent, session: ClaudeChatSession) => {
    event.preventDefault();
    event.stopPropagation();
    if (session.running) onAskClose(session.id);
    else onClose(session.id);
  };

  return (
    <div className="lumio-claude-tabs">
      <div className="lumio-claude-tabs-strip" role="tablist" aria-label="对话">
        {sessions.map((session) => {
          const on = session.id === activeSessionId;
          const title =
            session.titleLocked && session.title
              ? clipWidth(session.title, SESSION_TITLE_WIDTH)
              : "新对话";
          return (
            <button
              aria-selected={on}
              className={`lumio-claude-tab${on ? " is-on" : ""}${session.running ? " is-run" : ""}`}
              key={session.id}
              onClick={() => select(session.id)}
              role="tab"
              type="button"
            >
              <span className="glyph" aria-hidden="true">
                <ChatIcon />
              </span>
              <span className="t">{title}</span>
              <span
                aria-label="关闭这个对话"
                className="x"
                onClick={(event) => closeTab(event, session)}
                role="button"
              >
                <CloseIcon />
              </span>
            </button>
          );
        })}
        <button className="lumio-claude-tab-new" aria-label="新建对话" onClick={create} type="button">
          <PlusIcon />
        </button>
      </div>
      {asking ? (
        <div className="lumio-claude-tab-ask" role="alertdialog">
          <span>这个对话正在跑，关掉就断了。</span>
          <button className="is-yes" onClick={onConfirmClose} type="button">
            还是关掉
          </button>
          <button className="is-no" onClick={onCancelClose} type="button">
            先留着
          </button>
        </div>
      ) : null}
    </div>
  );
}

function ChatIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round">
      <path d="M14 7.4c0 2.8-2.7 5-6 5-.7 0-1.4-.1-2-.3L3 13.4l.8-2.3C2.7 10.2 2 8.9 2 7.4c0-2.8 2.7-5 6-5s6 2.2 6 5Z" />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
      <path d="M4.6 4.6l6.8 6.8M11.4 4.6l-6.8 6.8" />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
      <path d="M8 3.6v8.8M3.6 8h8.8" />
    </svg>
  );
}
