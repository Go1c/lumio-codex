import type { CSSProperties, ReactNode } from "react";
import { motion, useReducedMotion } from "motion/react";

/** jsdom / 老环境没有 IntersectionObserver：直接静态呈现，不做滚动触发。 */
const canObserve = typeof IntersectionObserver !== "undefined";

export interface RevealProps {
  children: ReactNode;
  /** 秒。同一组元素用递增 delay 形成 stagger 入场。 */
  delay?: number;
  /** 上浮距离（px）。 */
  y?: number;
  /** true = 挂载即入场（首屏 hero）；false = 滚动进入视口才入场。 */
  immediate?: boolean;
  className?: string;
  style?: CSSProperties;
}

/** 统一的进场动效包装器：淡入上浮，尊重 prefers-reduced-motion。 */
export function Reveal({
  children,
  delay = 0,
  y = 26,
  immediate = false,
  className,
  style,
}: RevealProps) {
  const reducedMotion = useReducedMotion();

  if (reducedMotion || (!immediate && !canObserve)) {
    return (
      <div className={className} style={style}>
        {children}
      </div>
    );
  }

  const transition = { duration: 0.7, delay, ease: [0.22, 1, 0.36, 1] as const };

  if (immediate) {
    return (
      <motion.div
        className={className}
        style={style}
        initial={{ opacity: 0, y }}
        animate={{ opacity: 1, y: 0 }}
        transition={transition}
      >
        {children}
      </motion.div>
    );
  }

  return (
    <motion.div
      className={className}
      style={style}
      initial={{ opacity: 0, y }}
      whileInView={{ opacity: 1, y: 0 }}
      viewport={{ once: true, margin: "-70px 0px" }}
      transition={transition}
    >
      {children}
    </motion.div>
  );
}
