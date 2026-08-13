import type { ReactNode } from "react";

import { useT } from "@/i18n";
import { ApiError, NetworkError } from "@/lib/api";
import { middleEllipsis } from "@/lib/format";

/** 表单顶部 / 页面级错误条：常驻直到解决，必须附带动作按钮（6.4 节）。 */
export function Banner({
  kind = "error",
  children,
  action,
  className = "",
}: {
  kind?: "error" | "warn" | "ok" | "info";
  children: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div className={`banner ${kind} ${className}`.trim()} role={kind === "error" ? "alert" : "status"}>
      <span>{children}</span>
      {action}
    </div>
  );
}

export function Spinner({ dark = false }: { dark?: boolean }) {
  return <span className={dark ? "spinner dark" : "spinner"} aria-hidden="true" />;
}

export function Skeleton({ height = 16, width, className = "" }: { height?: number; width?: number | string; className?: string }) {
  return (
    <div
      className={`skeleton ${className}`.trim()}
      style={{ height, width: width ?? "100%" }}
      aria-hidden="true"
    />
  );
}

/** 五态里的 loading：骨架块 + 供屏幕阅读器识别的状态文本。 */
export function LoadingBlock({ label, lines = 3 }: { label?: string; lines?: number }) {
  const t = useT();
  return (
    <div className="loading-block" role="status" aria-live="polite" aria-busy="true">
      <span className="sr-only">{label ?? t("common.loading")}</span>
      {Array.from({ length: lines }).map((_, index) => (
        <Skeleton key={index} height={index === 0 ? 20 : 14} width={index === 0 ? "45%" : undefined} />
      ))}
    </div>
  );
}

/** 从任意异常里取出可展示文案：业务错误用服务端 message，网络错误用本地文案。 */
export function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof NetworkError) return error.message;
  return fallback;
}

/** 五态里的 error：一句原因 + 一个「重试」动作，不留死胡同（1.2 节原则 3）。 */
export function ErrorBlock({
  error,
  fallback,
  onRetry,
}: {
  error?: unknown;
  fallback: string;
  onRetry?: () => void;
}) {
  const t = useT();
  return (
    <Banner
      kind="error"
      action={
        onRetry && (
          <button type="button" className="btn btn-secondary" onClick={onRetry}>
            {t("common.retry")}
          </button>
        )
      }
    >
      {errorMessage(error, fallback)}
    </Banner>
  );
}

/** 五态里的 empty：图标 + 一句说明 + 可选行动，不出现纯空白（6.4 节）。 */
export function EmptyBlock({ icon = "📭", text, action }: { icon?: string; text: string; action?: ReactNode }) {
  return (
    <div className="empty-block">
      <span className="empty-icon" aria-hidden="true">
        {icon}
      </span>
      <p>{text}</p>
      {action}
    </div>
  );
}

/**
 * 6.6 节：长邮箱 / 长路径中间省略号截断，hover（与聚焦）显示全文。
 */
export function Truncated({ text, max = 28, className = "" }: { text: string; max?: number; className?: string }) {
  const short = middleEllipsis(text, max);
  return (
    <span className={`truncated ${className}`.trim()} title={text} tabIndex={short === text ? -1 : 0}>
      {short}
    </span>
  );
}

/** 状态点：颜色之外必须有文字标签（6.6 节）。 */
export function StatusDot({ tone, label }: { tone: "green" | "blue" | "orange" | "gray"; label: string }) {
  return (
    <span className="status-dot-wrap">
      <span className={`status-dot ${tone}`} aria-hidden="true" />
      <span>{label}</span>
    </span>
  );
}

export function SectionCard({
  title,
  id,
  children,
  className = "",
}: {
  title: string;
  id?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section id={id} className={`acct-section ${className}`.trim()} aria-labelledby={id ? `${id}-title` : undefined}>
      <h3 id={id ? `${id}-title` : undefined}>{title}</h3>
      {children}
    </section>
  );
}
