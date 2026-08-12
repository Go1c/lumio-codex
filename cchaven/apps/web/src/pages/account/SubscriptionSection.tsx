import { useState } from "react";

import { checkout, getPlan } from "@/api/endpoints";
import type { Entitlement, Plan } from "@/api/types";
import { ErrorBlock, LoadingBlock, SectionCard, Spinner, errorMessage } from "@/components/ui";
import { useResource } from "@/hooks/useResource";
import { useT, type MessageKey } from "@/i18n";
import { channelLabel, formatDate, formatMoney } from "@/lib/format";
import { useSession } from "@/state/session";

/** 订阅徽标：状态点之外必须有文字（6.6 节）。 */
export function EntitlementBadge({ entitlement }: { entitlement: Entitlement | undefined }) {
  const t = useT();
  if (!entitlement) return null;

  switch (entitlement.status) {
    case "active":
      return (
        <span className="badge blue">
          {t("account.status_active", {
            date: formatDate(entitlement.expires_at),
            n: entitlement.days_left,
          })}
        </span>
      );
    case "trialing":
      return <span className="badge green">{t("account.status_trialing", { n: entitlement.days_left })}</span>;
    case "expired":
      return <span className="badge orange">{t("account.status_expired")}</span>;
    default:
      return <span className="badge gray">{t("account.status_none")}</span>;
  }
}

function checkoutLabelKey(status: Entitlement["status"] | undefined): MessageKey {
  if (status === "trialing") return "account.checkout_trial";
  if (status === "active") return "account.checkout";
  return "account.checkout_start";
}

/**
 * 5.6「订阅与付款」：付款只在官网，跳支付服务商托管页，站内不收集任何卡号。
 * 五态：loading 骨架 / error + 重试 / disabled（下单中）/ empty 与无权限不适用。
 */
export function SubscriptionSection() {
  const t = useT();
  const { entitlement } = useSession();

  const plan = useResource<Plan>((signal) => getPlan(signal), []);
  const [channel, setChannel] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const channels = plan.data?.channels ?? [];
  const selectedChannel = channel || channels[0] || "";

  async function startCheckout() {
    if (!selectedChannel || busy) return;

    setBusy(true);
    setError("");
    try {
      const result = await checkout(selectedChannel);
      // 托管收银台由支付服务商提供，本站不接触任何卡号。
      window.location.assign(result.pay_url);
    } catch (err) {
      setError(errorMessage(err, t("common.unknown_error")));
      setBusy(false);
    }
  }

  return (
    <SectionCard id="billing" title={t("account.billing")}>
      {plan.status === "loading" && <LoadingBlock lines={2} />}

      {plan.status === "error" && (
        <ErrorBlock error={plan.error} fallback={t("account.load_error")} onRetry={plan.reload} />
      )}

      {plan.status === "success" && plan.data && (
        <>
          <div className="sub-row">
            <EntitlementBadge entitlement={entitlement} />
            <span className="note">
              {t("account.plan_price", {
                price: formatMoney(plan.data.amount_cents, plan.data.currency),
              })}
            </span>
          </div>

          {error && <ErrorBlock error={undefined} fallback={error} onRetry={() => void startCheckout()} />}

          <div className="sub-row">
            {channels.length > 1 && (
              <label className="pay-channel">
                <span className="sr-only">{t("account.pay_channel")}</span>
                <select
                  value={selectedChannel}
                  onChange={(event) => setChannel(event.target.value)}
                  disabled={busy}
                  aria-label={t("account.pay_channel")}
                >
                  {channels.map((item) => (
                    <option key={item} value={item}>
                      {channelLabel(item)}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void startCheckout()}
              disabled={busy || !selectedChannel}
            >
              {busy && <Spinner />}
              {busy ? t("account.checkout_busy") : t(checkoutLabelKey(entitlement?.status))}
            </button>
          </div>

          <p className="note">{t("account.pay_note")}</p>
        </>
      )}
    </SectionCard>
  );
}
