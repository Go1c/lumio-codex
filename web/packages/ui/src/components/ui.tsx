import type { ReactNode } from "react";

/** 页面级 / 表单级提示条。错误抢读（alert），其余不打断当前操作（status）。 */
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
    <div
      className={`banner ${kind} ${className}`.trim()}
      role={kind === "error" ? "alert" : "status"}
    >
      <span>{children}</span>
      {action}
    </div>
  );
}

export function Spinner({ dark = false }: { dark?: boolean }) {
  return <span className={dark ? "spinner dark" : "spinner"} aria-hidden="true" />;
}

export function Skeleton({
  height = 16,
  width,
  className = "",
}: {
  height?: number;
  width?: number | string;
  className?: string;
}) {
  return (
    <div
      className={`skeleton ${className}`.trim()}
      style={{ height, width: width ?? "100%" }}
      aria-hidden="true"
    />
  );
}

/** 加载态：骨架块 + 供屏幕阅读器识别的状态文本。 */
export function LoadingBlock({ label = "加载中…", lines = 3 }: { label?: string; lines?: number }) {
  return (
    <div className="loading-block" role="status" aria-live="polite" aria-busy="true">
      <span className="sr-only">{label}</span>
      {Array.from({ length: lines }).map((_, index) => (
        <Skeleton
          key={index}
          height={index === 0 ? 20 : 14}
          width={index === 0 ? "45%" : undefined}
        />
      ))}
    </div>
  );
}

/** 错误态：一句原因 + 一个「重试」动作，不留死胡同。 */
export function ErrorBlock({
  message,
  onRetry,
  retryLabel = "重试",
}: {
  message: string;
  onRetry?: () => void;
  retryLabel?: string;
}) {
  return (
    <Banner
      kind="error"
      action={
        onRetry && (
          <button type="button" className="btn btn-secondary" onClick={onRetry}>
            {retryLabel}
          </button>
        )
      }
    >
      {message}
    </Banner>
  );
}

/** 空态：图标 + 一句说明 + 可选行动，不出现纯空白。 */
export function EmptyBlock({
  icon = "📭",
  text,
  action,
}: {
  icon?: string;
  text: string;
  action?: ReactNode;
}) {
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

/** 长邮箱 / 长路径中间省略，hover 与聚焦显示全文。 */
export function Truncated({
  text,
  max = 28,
  className = "",
}: {
  text: string;
  max?: number;
  className?: string;
}) {
  const short = middleEllipsis(text, max);
  return (
    <span className={`truncated ${className}`.trim()} title={text} tabIndex={short === text ? -1 : 0}>
      {short}
    </span>
  );
}

export function middleEllipsis(text: string, max: number): string {
  if (text.length <= max) return text;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

/** 状态点：颜色之外必须有文字标签。 */
export function StatusDot({
  tone,
  label,
}: {
  tone: "green" | "blue" | "orange" | "gray";
  label: string;
}) {
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
    <section
      id={id}
      className={`acct-section ${className}`.trim()}
      aria-labelledby={id ? `${id}-title` : undefined}
    >
      <h3 id={id ? `${id}-title` : undefined}>{title}</h3>
      {children}
    </section>
  );
}
