import { Check, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { lumioErrorLabel } from "../errors.ts";
import { LumioCommandError, isSessionExpired, runProvisioningStep } from "../invoke.ts";
import {
  ACCOUNT_INSUFFICIENT_BALANCE_CODE,
  PROVISIONING_STEP_IDS,
  PROVISIONING_STEP_TITLES,
} from "../state.ts";
import type { LumioProvisioning, ProvisioningStepId } from "../state.ts";
import type { LumioAccountSummary } from "../types.ts";

const SLOW_STEP_MS = 10_000;
const SETTLE_MS = 600;

const STEP_DETAILS: Record<ProvisioningStepId, string> = {
  "verify-account": "确认登录状态与账户资格",
  "prepare-connection": "初始化本机的服务连接",
  "sync-models": "获取服务端提供的模型列表",
  "write-config": "先备份原始配置，再原子写入",
};

function errorCodeOf(error: unknown): string {
  return error instanceof LumioCommandError ? error.errorCode : "UNKNOWN";
}

interface ProvisioningViewProps {
  email: string | null;
  provisioning: LumioProvisioning;
  canPay: boolean;
  onPay: () => void;
  onStepStarted: (step: ProvisioningStepId) => void;
  onStepCompleted: (step: ProvisioningStepId) => void;
  onStepFailed: (step: ProvisioningStepId, errorCode: string) => void;
  onAccountResolved: (account: LumioAccountSummary) => void;
  onCompleted: () => void;
  onDeferred: () => void;
}

export function ProvisioningView({
  email,
  provisioning,
  canPay,
  onPay,
  onStepStarted,
  onStepCompleted,
  onStepFailed,
  onAccountResolved,
  onCompleted,
  onDeferred,
}: ProvisioningViewProps) {
  const [slowStep, setSlowStep] = useState<ProvisioningStepId | null>(null);
  const running = useRef(false);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  const runFrom = useCallback(
    async (startIndex: number) => {
      if (running.current) return;
      running.current = true;
      try {
        for (const step of PROVISIONING_STEP_IDS.slice(startIndex)) {
          onStepStarted(step);
          const slowTimer = setTimeout(() => setSlowStep(step), SLOW_STEP_MS);
          timers.current.push(slowTimer);
          try {
            // `verify-account` 是这轮唯一一次真实拉取账户；不接住它，首页只有 bootstrap 的占位值。
            const result = await runProvisioningStep(step);
            // Rust 旧包可能漏掉 account 字段；`undefined !== null` 会把假账户推进首页导致黑屏。
            if (result.account) onAccountResolved(result.account);
          } catch (error: unknown) {
            // 会话过期已由全局监听器处理（回到登录入口）；这里再报步骤失败会把
            // 用户从登录页拽回 provisioning，形成「重试→再过期」死循环。
            if (isSessionExpired(error)) return;
            onStepFailed(step, errorCodeOf(error));
            return;
          } finally {
            clearTimeout(slowTimer);
            setSlowStep(null);
          }
          onStepCompleted(step);
        }
        // Hold the finished checkmarks briefly so the last step is perceivable.
        timers.current.push(setTimeout(onCompleted, SETTLE_MS));
      } finally {
        running.current = false;
      }
    },
    [onAccountResolved, onCompleted, onStepCompleted, onStepFailed, onStepStarted],
  );

  useEffect(() => {
    void runFrom(0);
    const pending = timers.current;
    return () => {
      for (const timer of pending) clearTimeout(timer);
    };
  }, [runFrom]);

  const failedStep = provisioning.failedStep;
  const failedIndex = failedStep === null ? -1 : PROVISIONING_STEP_IDS.indexOf(failedStep);
  const payable =
    provisioning.errorCode === ACCOUNT_INSUFFICIENT_BALANCE_CODE && canPay;

  return (
    <section aria-live="polite" className="lumio-provision">
      <span className="lumio-app-icon is-card" aria-hidden="true">
        <img alt="" src="/lumio-icon.png" />
      </span>
      <h1>正在准备</h1>
      {email === null ? null : <p className="lumio-provision-lead">{email}</p>}

      <ol className="lumio-steps">
        {PROVISIONING_STEP_IDS.map((step, index) => {
          const status = provisioning.steps[step];
          return (
            <li className={`lumio-step is-${status}`} key={step}>
              <span className="lumio-step-dot">
                {status === "done" ? (
                  <Check size={15} />
                ) : status === "failed" ? (
                  <X size={15} />
                ) : (
                  index + 1
                )}
              </span>
              <span className="lumio-step-body">
                <strong>{PROVISIONING_STEP_TITLES[step]}</strong>
                <small>
                  {status === "failed" && provisioning.errorCode !== null
                    ? lumioErrorLabel(provisioning.errorCode)
                    : slowStep === step
                      ? "比平时慢一些，仍在继续…"
                      : STEP_DETAILS[step]}
                </small>
              </span>
              <span className="lumio-step-state">
                {status === "running" ? "进行中…" : status === "done" ? "完成" : ""}
              </span>
            </li>
          );
        })}
      </ol>

      {failedStep === null ? (
        <p className="lumio-provision-foot">不需要手动操作，完成后自动进入首页</p>
      ) : (
        <>
          {/* 余额不足是账户态：主按钮带用户去充值，重试只是充完值后的恢复路径。 */}
          {payable ? (
            <div className="lumio-provision-actions">
              <button className="lumio-button is-primary" onClick={onPay} type="button">
                去充值
              </button>
              <button
                className="lumio-button is-secondary"
                onClick={() => void runFrom(failedIndex)}
                type="button"
              >
                重试
              </button>
              <button className="lumio-button is-secondary" onClick={onDeferred} type="button">
                稍后处理
              </button>
            </div>
          ) : (
            <div className="lumio-provision-actions">
              <button
                className="lumio-button is-primary"
                onClick={() => void runFrom(failedIndex)}
                type="button"
              >
                重试
              </button>
              <button className="lumio-button is-secondary" onClick={onDeferred} type="button">
                稍后处理
              </button>
            </div>
          )}
          <p className="lumio-provision-foot">
            {payable
              ? "充值完成后回到这里重试，本机配置不会被修改。"
              : provisioning.suggestRepair
                ? "多次尝试仍未成功，可以到修复页检查本机配置。"
                : "遇到问题时你的本机配置不会被修改。"}
          </p>
        </>
      )}
    </section>
  );
}
