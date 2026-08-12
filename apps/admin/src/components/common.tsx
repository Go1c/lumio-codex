import { t } from "../i18n";

/** 页面级错误条：常驻直到解决，必须附带动作按钮（交互设计 6.4）。 */
export function ErrorBanner({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="banner error" role="alert">
      <span>{message}</span>
      {onRetry && (
        <button type="button" className="btn btn-secondary btn-sm" onClick={onRetry}>
          {t("common.retry")}
        </button>
      )}
    </div>
  );
}

/** 表格骨架屏，覆盖五态里的 loading。 */
export function TableSkeleton({ rows = 5, cols = 6 }: { rows?: number; cols?: number }) {
  return (
    <tbody aria-hidden="true" data-testid="table-skeleton">
      {Array.from({ length: rows }, (_, r) => (
        <tr key={r}>
          {Array.from({ length: cols }, (_, c) => (
            <td key={c}>
              <span className="skeleton skeleton-line" />
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  );
}

export function CardSkeleton() {
  return <div className="skeleton skeleton-card" aria-hidden="true" data-testid="card-skeleton" />;
}

type TagTone = "blue" | "green" | "gray" | "red" | "orange";

export function Tag({ tone, label }: { tone: TagTone; label: string }) {
  return <span className={`tag t-${tone}`}>{label}</span>;
}

interface ChipsProps<T extends string> {
  label: string;
  options: { value: T; label: string }[];
  value: T;
  disabled?: boolean;
  onChange: (value: T) => void;
}

/** 筛选 chips。用 aria-pressed 表达选中态，键盘可 Tab 逐个聚焦。 */
export function Chips<T extends string>({
  label,
  options,
  value,
  disabled,
  onChange,
}: ChipsProps<T>) {
  return (
    <div className="chip-group" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          className={`chip ${value === option.value ? "on" : ""}`}
          aria-pressed={value === option.value}
          disabled={disabled}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

interface PaginationProps {
  page: number;
  pageSize: number;
  total: number;
  disabled?: boolean;
  onChange: (page: number) => void;
}

export function Pagination({ page, pageSize, total, disabled, onChange }: PaginationProps) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  if (total === 0) return null;

  return (
    <div className="pager">
      <span className="pager-info">
        {t("common.pageInfo", { page, pages, total })}
      </span>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        disabled={disabled || page <= 1}
        onClick={() => onChange(page - 1)}
      >
        {t("common.prevPage")}
      </button>
      <button
        type="button"
        className="btn btn-secondary btn-sm"
        disabled={disabled || page >= pages}
        onClick={() => onChange(page + 1)}
      >
        {t("common.nextPage")}
      </button>
    </div>
  );
}
