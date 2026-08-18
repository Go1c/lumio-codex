import { useEffect, useId, useRef, useState } from "react";

import { supportChannels } from "../config";

function CloseIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path
        d="M18 6 6 18M6 6l12 12"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** 三站共用的右下角客服气泡。初版只放 QQ 群号与飞书群外链，交互对齐 Workflow 的 launcher。 */
export function SupportBubble() {
  const { qqGroupNumber, feishuGroupUrl } = supportChannels();
  const [open, setOpen] = useState(false);
  const [qqCopied, setQqCopied] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const launcherRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const titleId = useId();

  useEffect(() => {
    if (!open) return;

    panelRef.current?.querySelector<HTMLElement>("button, a[href]")?.focus();

    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setOpen(false);
      launcherRef.current?.focus();
    }

    function onPointerDown(event: MouseEvent) {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    }

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mousedown", onPointerDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mousedown", onPointerDown);
    };
  }, [open]);

  useEffect(
    () => () => {
      clearTimeout(copyTimer.current);
    },
    [],
  );

  if (!qqGroupNumber && !feishuGroupUrl) return null;

  async function copyQqNumber() {
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(qqGroupNumber);
      setQqCopied(true);
      clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setQqCopied(false), 1600);
    } catch {
      setQqCopied(false);
    }
  }

  return (
    <div className="support-bubble" ref={rootRef}>
      {open ? (
        <div
          ref={panelRef}
          className="support-bubble-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby={titleId}
          aria-label="Lumio 支持"
        >
          <div className="support-bubble-head">
            <div>
              <div id={titleId} className="support-bubble-title">
                Lumio 支持
              </div>
              <p className="support-bubble-greeting">你好，有什么可以帮到你？</p>
            </div>
            <button
              type="button"
              className="support-bubble-close"
              aria-label="关闭"
              onClick={() => {
                setOpen(false);
                launcherRef.current?.focus();
              }}
            >
              <CloseIcon />
            </button>
          </div>
          <div className="support-bubble-body">
            {qqGroupNumber ? (
              <button
                type="button"
                className="support-bubble-card support-bubble-card-qq"
                aria-label={`复制 QQ 群号 ${qqGroupNumber}`}
                onClick={() => {
                  void copyQqNumber();
                }}
              >
                <span className="support-bubble-card-mark" aria-hidden="true">
                  QQ
                </span>
                <span>
                  <strong>QQ 群 {qqGroupNumber}</strong>
                  <span className="support-bubble-card-desc">
                    {qqCopied ? "已复制到剪贴板" : "点击复制群号"}
                  </span>
                </span>
              </button>
            ) : null}
            {feishuGroupUrl ? (
              <a
                className="support-bubble-card support-bubble-card-feishu"
                href={feishuGroupUrl}
                target="_blank"
                rel="noreferrer"
              >
                <span className="support-bubble-card-mark" aria-hidden="true">
                  飞
                </span>
                <span>
                  <strong>加入飞书群</strong>
                  <span className="support-bubble-card-desc">产品团队直接答疑</span>
                </span>
              </a>
            ) : null}
          </div>
        </div>
      ) : null}
      <button
        ref={launcherRef}
        type="button"
        className="support-bubble-launcher"
        title="客服与反馈"
        aria-label="客服与反馈"
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen((current) => !current)}
      >
        {open ? (
          <CloseIcon />
        ) : (
          <span className="support-bubble-dots" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
        )}
      </button>
    </div>
  );
}
