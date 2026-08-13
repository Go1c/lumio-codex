import { useEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent } from "react";

import { useT } from "@/i18n";

const LENGTH = 6;
const EMPTY = Array.from({ length: LENGTH }, () => "");

/**
 * 6 格验证码输入（4.6 节）：自动跳位、退格回退、粘贴自动分配、填满自动提交。
 * 无障碍：每格 aria-label 逐格播报「第 N 位，共 6 位」，另有 aria-live 汇报整体进度。
 */
export function CodeInput({
  disabled = false,
  errorNonce = 0,
  onComplete,
}: {
  disabled?: boolean;
  /** 每次校验失败自增：用于清空格子并触发抖动动画。 */
  errorNonce?: number;
  onComplete: (code: string) => void;
}) {
  const t = useT();
  const [digits, setDigits] = useState<string[]>(EMPTY);
  const refs = useRef<(HTMLInputElement | null)[]>([]);
  const firstErrorRender = useRef(true);

  useEffect(() => {
    if (firstErrorRender.current) {
      firstErrorRender.current = false;
      return;
    }
    setDigits(EMPTY);
    refs.current[0]?.focus();
  }, [errorNonce]);

  function fill(values: string[], from: number) {
    const next = [...digits];
    values.forEach((char, offset) => {
      if (from + offset < LENGTH) next[from + offset] = char;
    });
    setDigits(next);

    const filled = next.filter(Boolean).length;
    if (next.every((digit) => digit !== "")) {
      onComplete(next.join(""));
    } else {
      refs.current[Math.min(filled, LENGTH - 1)]?.focus();
    }
  }

  function handleChange(index: number, raw: string) {
    const chars = raw.replace(/\D/g, "").split("");
    if (chars.length === 0) {
      const next = [...digits];
      next[index] = "";
      setDigits(next);
      return;
    }
    fill(chars.slice(0, LENGTH - index), index);
  }

  function handleKeyDown(index: number, event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Backspace" && !digits[index] && index > 0) {
      event.preventDefault();
      const next = [...digits];
      next[index - 1] = "";
      setDigits(next);
      refs.current[index - 1]?.focus();
    }
    if (event.key === "ArrowLeft" && index > 0) refs.current[index - 1]?.focus();
    if (event.key === "ArrowRight" && index < LENGTH - 1) refs.current[index + 1]?.focus();
  }

  function handlePaste(index: number, event: ClipboardEvent<HTMLInputElement>) {
    const text = event.clipboardData.getData("text").replace(/\D/g, "");
    if (!text) return;
    event.preventDefault();
    fill(text.slice(0, LENGTH - index).split(""), index);
  }

  const filledCount = digits.filter(Boolean).length;

  return (
    <>
      <div className={`code-boxes ${errorNonce > 0 ? "error" : ""}`.trim()}>
        {digits.map((digit, index) => (
          <input
            key={index}
            ref={(element) => {
              refs.current[index] = element;
            }}
            value={digit}
            inputMode="numeric"
            autoComplete={index === 0 ? "one-time-code" : "off"}
            maxLength={LENGTH}
            disabled={disabled}
            aria-label={t("verify.box_label", { i: index + 1 })}
            onChange={(event) => handleChange(index, event.target.value)}
            onKeyDown={(event) => handleKeyDown(index, event)}
            onPaste={(event) => handlePaste(index, event)}
          />
        ))}
      </div>
      <p className="sr-only" aria-live="polite">
        {t("verify.progress", { n: filledCount })}
      </p>
    </>
  );
}
