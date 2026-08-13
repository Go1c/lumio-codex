import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import type { Bucket, DailyCount, Distributions, MetricCard, MetricsOverview } from "../api/types";
import { useAuth } from "../auth/AuthProvider";
import { CardSkeleton, ErrorBanner } from "../components/common";
import { t } from "../i18n";
import {
  DASH,
  formatAmountCompact,
  formatCount,
  formatDayAxis,
  formatDelta,
  formatRate,
} from "../lib/format";

type Tone = "neutral" | "up" | "down";

interface CardView {
  key: string;
  label: string;
  value: string;
  sub: string;
  tone: Tone;
}

/** delta 缺失时不上色，避免用绿色暗示一个并不存在的增长。 */
function toneOf(delta: number | null | undefined): Tone {
  if (delta === null || delta === undefined) return "neutral";
  return delta < 0 ? "down" : "up";
}

/** 缺数（value 为 null）一律显示「—」，绝不回落成 0。 */
function cardValue(card: MetricCard, render: (value: number) => string): string {
  return card.value === null || card.value === undefined ? DASH : render(card.value);
}

function buildCards(overview: MetricsOverview): CardView[] {
  return [
    {
      key: "dau",
      label: t("dash.card.dau"),
      value: cardValue(overview.dau, formatCount),
      sub: t("dash.sub.dau", { delta: formatDelta(overview.dau.delta) }),
      tone: toneOf(overview.dau.delta),
    },
    {
      key: "signups",
      label: t("dash.card.signups"),
      value: cardValue(overview.signups, formatCount),
      sub: t("dash.sub.signups", { n: formatCount(overview.signups.secondary) }),
      tone: "neutral",
    },
    {
      key: "subscribers",
      label: t("dash.card.subscribers"),
      value: cardValue(overview.subscribers, formatCount),
      sub: t("dash.sub.subscribers", { n: formatCount(overview.subscribers.secondary) }),
      tone: "neutral",
    },
    {
      key: "revenue",
      label: t("dash.card.revenue"),
      // 后端给的是分，卡片按元展示。
      value: cardValue(overview.revenue, (cents) => `¥${formatAmountCompact(cents)}`),
      sub: t("dash.sub.revenue", { n: formatCount(overview.revenue.secondary) }),
      tone: "neutral",
    },
    {
      key: "trial_conversion",
      label: t("dash.card.conversion"),
      value: cardValue(overview.trial_conversion, formatRate),
      sub: t("dash.sub.conversion"),
      tone: "neutral",
    },
    {
      key: "retention_d7",
      label: t("dash.card.retention"),
      value: cardValue(overview.retention_d7, formatRate),
      // 下降转橙色（交互设计 7.1）。
      sub: t("dash.sub.retention", { delta: formatDelta(overview.retention_d7.delta) }),
      tone: toneOf(overview.retention_d7.delta),
    },
  ];
}

function DauChart({ items }: { items: DailyCount[] }) {
  const max = items.reduce((acc, item) => Math.max(acc, item.count), 0);

  return (
    <div className="bar-chart">
      {items.map((item, index) => {
        const isToday = index === items.length - 1;
        const label = isToday ? t("dash.dau7d.today") : formatDayAxis(item.day);
        const height = max > 0 ? Math.max(4, (item.count / max) * 100) : 4;
        return (
          <div className="bar-col" key={item.day} aria-label={`${label}：${formatCount(item.count)}`}>
            <div className="bar-v">{formatCount(item.count)}</div>
            <div className="bar" style={{ height: `${height}%` }} />
            <div className="bar-d">{label}</div>
          </div>
        );
      })}
    </div>
  );
}

function DistributionBars({ buckets, variant }: { buckets: Bucket[]; variant: string }) {
  const total = buckets.reduce((acc, bucket) => acc + bucket.count, 0);
  if (buckets.length === 0 || total === 0) {
    return <p className="muted">{t("dash.dist.empty")}</p>;
  }

  return (
    <>
      {buckets.map((bucket) => {
        const pct = Math.round((bucket.count / total) * 100);
        return (
          <div className="dist-row" key={bucket.label}>
            <span className="dl">{bucket.label}</span>
            <div className="dist-track">
              <div className={`dist-fill dist-${variant}`} style={{ width: `${pct}%` }} />
            </div>
            <span className="dp">{pct}%</span>
          </div>
        );
      })}
    </>
  );
}

export function DashboardPage() {
  const { handleApiError } = useAuth();
  const [overview, setOverview] = useState<MetricsOverview | null>(null);
  const [dau, setDau] = useState<DailyCount[]>([]);
  const [distributions, setDistributions] = useState<Distributions | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [overviewData, dauData, distributionData] = await Promise.all([
        api.fetchOverview(),
        api.fetchDau(7),
        api.fetchDistributions(30),
      ]);
      setOverview(overviewData);
      setDau(dauData.items ?? []);
      setDistributions(distributionData);
    } catch (err) {
      if (!handleApiError(err)) {
        setError(
          t("dash.loadFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setLoading(false);
    }
  }, [handleApiError]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="adm-page">
      <h1>{t("dash.title")}</h1>

      {error && <ErrorBanner message={error} onRetry={() => void load()} />}

      {loading && (
        <div className="stat-grid">
          {Array.from({ length: 6 }, (_, index) => (
            <CardSkeleton key={index} />
          ))}
        </div>
      )}

      {!loading && overview && (
        <>
          <div className="stat-grid">
            {buildCards(overview).map((card) => (
              <div className="stat-card" key={card.key} data-testid={`stat-${card.key}`}>
                <div className="lb">{card.label}</div>
                <div className="v">{card.value}</div>
                <div className={`sub tone-${card.tone}`}>{card.sub}</div>
              </div>
            ))}
          </div>

          <div className="adm-cols">
            <section className="adm-card">
              <h2>{t("dash.dau7d")}</h2>
              <DauChart items={dau} />
            </section>

            <section className="adm-card">
              <h2>{t("dash.dist.platform")}</h2>
              <DistributionBars buckets={distributions?.platform ?? []} variant="platform" />

              <h2 className="spaced">{t("dash.dist.version")}</h2>
              <DistributionBars buckets={distributions?.app_version ?? []} variant="version" />

              <h2 className="spaced">{t("dash.dist.source")}</h2>
              <DistributionBars buckets={distributions?.source ?? []} variant="source" />
            </section>
          </div>
        </>
      )}
    </div>
  );
}
