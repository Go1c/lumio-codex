import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import * as api from "../api/endpoints";
import { canEditOpsConfig, CONFIG_KEYS, type OpsConfig } from "../api/types";
import { useAuth } from "../auth/AuthProvider";
import { ErrorBanner } from "../components/common";
import { useToast } from "../components/ToastProvider";
import { t } from "../i18n";
import { AuditLogSection } from "./AuditLogSection";

interface FormState {
  rewardDays: string;
  trialDays: string;
  priceYuan: string;
}

interface FieldErrors {
  rewardDays?: string;
  trialDays?: string;
  priceYuan?: string;
}

function toForm(config: OpsConfig): FormState {
  return {
    rewardDays: String(config.invite_reward_days),
    trialDays: String(config.invite_trial_days),
    priceYuan: (config.pricing_monthly.amount_cents / 100).toFixed(2).replace(/\.00$/, ""),
  };
}

function validate(form: FormState): FieldErrors {
  const errors: FieldErrors = {};
  if (!/^\d+$/.test(form.rewardDays)) errors.rewardDays = t("settings.invalidInt");
  if (!/^\d+$/.test(form.trialDays) || Number(form.trialDays) < 1) {
    errors.trialDays = t("settings.invalidTrial");
  }
  if (!/^\d+(\.\d{1,2})?$/.test(form.priceYuan) || Number(form.priceYuan) <= 0) {
    errors.priceYuan = t("settings.invalidPrice");
  }
  return errors;
}

/**
 * 运营配置：前台文案与数值一律从这里下发（交互设计 7.4）。审计日志作为本页子区块。
 *
 * 只读角色（support）看到的是同一页的只读版本，而不是整页 403：
 * 客服回答「现在奖励几天、包月多少钱」需要这些数字，读配置本来也是他有权做的事。
 */
export function SettingsPage() {
  const { handleApiError, me } = useAuth();
  const { toast } = useToast();

  const canEdit = canEditOpsConfig(me?.role ?? "");

  const [config, setConfig] = useState<OpsConfig | null>(null);
  const [form, setForm] = useState<FormState>({ rewardDays: "", trialDays: "", priceYuan: "" });
  const [errors, setErrors] = useState<FieldErrors>({});
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [saveError, setSaveError] = useState("");
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError("");
    try {
      const data = await api.fetchConfigs();
      setConfig(data);
      setForm(toForm(data));
    } catch (err) {
      if (!handleApiError(err)) {
        setLoadError(
          t("settings.loadFailed", {
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

  function update(field: keyof FormState, value: string) {
    setForm((current) => ({ ...current, [field]: value }));
    // 出错后转为输入即校验（edit-to-clear，交互设计 6.1）。
    if (errors[field]) setErrors((current) => ({ ...current, [field]: undefined }));
  }

  async function onSubmit(event: React.FormEvent) {
    event.preventDefault();
    if (!config || !canEdit) return;

    const nextErrors = validate(form);
    setErrors(nextErrors);
    if (Object.values(nextErrors).some(Boolean)) return;

    // 只提交改动过的项，审计日志才不会被无变化的写入淹没。
    const values: Record<string, unknown> = {};
    const rewardDays = Number(form.rewardDays);
    const trialDays = Number(form.trialDays);
    const amountCents = Math.round(Number(form.priceYuan) * 100);

    if (rewardDays !== config.invite_reward_days) values[CONFIG_KEYS.inviteRewardDays] = rewardDays;
    if (trialDays !== config.invite_trial_days) values[CONFIG_KEYS.inviteTrialDays] = trialDays;
    if (amountCents !== config.pricing_monthly.amount_cents) {
      values[CONFIG_KEYS.pricingMonthly] = {
        amount_cents: amountCents,
        currency: config.pricing_monthly.currency,
      };
    }
    if (Object.keys(values).length === 0) return;

    setSaving(true);
    setSaveError("");
    try {
      const updated = await api.saveConfigs(values);
      setConfig(updated);
      setForm(toForm(updated));
      toast(t("settings.saved"));
    } catch (err) {
      // 403 就地呈现：会话没问题，只是这项操作不对当前角色开放。
      if (err instanceof ApiError && err.isForbidden) {
        setSaveError(t("settings.readOnly"));
      } else if (!handleApiError(err)) {
        setSaveError(
          t("settings.saveFailed", {
            message: err instanceof ApiError ? err.message : t("error.generic"),
          }),
        );
      }
    } finally {
      setSaving(false);
    }
  }

  const rewardOff = form.rewardDays.trim() === "0";

  return (
    <div className="adm-page narrow">
      <h1>{t("settings.title")}</h1>
      <p className="muted">{t("settings.intro")}</p>

      {!canEdit && (
        <p className="adm-hint" id="config-denied-hint" role="note">
          {t("settings.readOnly")}
        </p>
      )}

      {loadError && <ErrorBanner message={loadError} onRetry={() => void load()} />}
      {saveError && <ErrorBanner message={saveError} />}

      {loading && <div className="skeleton skeleton-card" aria-hidden="true" />}

      {!loading && config && (
        <form onSubmit={onSubmit} noValidate>
          <fieldset disabled={saving}>
            <section className="adm-card">
              <h2>{t("settings.invite")}</h2>

              <div className="field">
                <label htmlFor="reward-days">{t("settings.rewardDays")}</label>
                <input
                  id="reward-days"
                  className={`narrow-input ${errors.rewardDays ? "invalid" : ""}`}
                  inputMode="numeric"
                  readOnly={!canEdit}
                  value={form.rewardDays}
                  aria-invalid={errors.rewardDays ? true : undefined}
                  aria-describedby={
                    errors.rewardDays ? "reward-days-hint reward-days-error" : "reward-days-hint"
                  }
                  onBlur={() => setErrors((current) => ({ ...current, ...validateField("rewardDays", form) }))}
                  onChange={(event) => update("rewardDays", event.target.value)}
                />
                <div className="hint" id="reward-days-hint">
                  {t("settings.rewardHint")}
                </div>
                {errors.rewardDays && (
                  <div className="err" id="reward-days-error">
                    {errors.rewardDays}
                  </div>
                )}
                {rewardOff && !errors.rewardDays && (
                  <div className="notice" role="note">
                    {t("settings.rewardOff")}
                  </div>
                )}
              </div>

              <div className="field last">
                <label htmlFor="trial-days">{t("settings.trialDays")}</label>
                <input
                  id="trial-days"
                  className={`narrow-input ${errors.trialDays ? "invalid" : ""}`}
                  inputMode="numeric"
                  readOnly={!canEdit}
                  value={form.trialDays}
                  aria-invalid={errors.trialDays ? true : undefined}
                  aria-describedby={
                    errors.trialDays ? "trial-days-hint trial-days-error" : "trial-days-hint"
                  }
                  onBlur={() => setErrors((current) => ({ ...current, ...validateField("trialDays", form) }))}
                  onChange={(event) => update("trialDays", event.target.value)}
                />
                <div className="hint" id="trial-days-hint">
                  {t("settings.trialHint")}
                </div>
                {errors.trialDays && (
                  <div className="err" id="trial-days-error">
                    {errors.trialDays}
                  </div>
                )}
              </div>
            </section>

            <section className="adm-card">
              <h2>{t("settings.pricing")}</h2>
              <div className="field last">
                <label htmlFor="price-monthly">{t("settings.monthly")}</label>
                <input
                  id="price-monthly"
                  className={`narrow-input ${errors.priceYuan ? "invalid" : ""}`}
                  inputMode="decimal"
                  readOnly={!canEdit}
                  value={form.priceYuan}
                  aria-invalid={errors.priceYuan ? true : undefined}
                  aria-describedby={
                    errors.priceYuan ? "price-monthly-hint price-monthly-error" : "price-monthly-hint"
                  }
                  onBlur={() => setErrors((current) => ({ ...current, ...validateField("priceYuan", form) }))}
                  onChange={(event) => update("priceYuan", event.target.value)}
                />
                <div className="hint" id="price-monthly-hint">
                  {t("settings.monthlyHint")}
                </div>
                {errors.priceYuan && (
                  <div className="err" id="price-monthly-error">
                    {errors.priceYuan}
                  </div>
                )}
              </div>
            </section>

            <button
              type="submit"
              className="btn btn-primary"
              aria-describedby={canEdit ? undefined : "config-denied-hint"}
              disabled={!canEdit}
            >
              {saving && <span className="spinner" />}
              {saving ? t("common.saving") : t("common.save")}
            </button>
          </fieldset>
        </form>
      )}

      <AuditLogSection />
    </div>
  );
}

/** 单字段 blur 校验，只回填该字段的错误。 */
function validateField(field: keyof FormState, form: FormState): FieldErrors {
  const all = validate(form);
  return { [field]: all[field] } as FieldErrors;
}
