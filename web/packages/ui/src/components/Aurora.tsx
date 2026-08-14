import { useMemo } from "react";

export type AuroraVariant = "brand" | "codex" | "claude";

interface Star {
  left: number;
  top: number;
  size: number;
  delay: number;
  duration: number;
  opacity: number;
}

/** 每次挂载生成一批伪随机星点；数量克制，闪烁周期错开。 */
function makeStars(count: number): Star[] {
  return Array.from({ length: count }, () => ({
    left: Math.random() * 100,
    top: Math.random() * 92,
    size: Math.random() < 0.25 ? 3 : 2,
    delay: Math.random() * 6,
    duration: 3.5 + Math.random() * 5,
    opacity: 0.25 + Math.random() * 0.5,
  }));
}

/**
 * 页面顶部的深空背景层：星点闪烁 + 弥散光晕（纯 CSS 动画，装饰性内容对读屏隐藏）。
 * 放在页面片段最前面即可；main 的层叠规则保证内容压在其上。
 */
export function Aurora({ variant = "brand" }: { variant?: AuroraVariant }) {
  const stars = useMemo(() => makeStars(26), []);

  return (
    <div className={`aurora aurora-${variant}`} aria-hidden="true">
      <span className="blob b1" />
      <span className="blob b2" />
      <span className="blob b3" />
      {stars.map((star, index) => (
        <span
          key={index}
          className="star"
          style={{
            left: `${star.left}%`,
            top: `${star.top}%`,
            width: star.size,
            height: star.size,
            animationDelay: `${star.delay}s`,
            animationDuration: `${star.duration}s`,
            opacity: star.opacity,
          }}
        />
      ))}
    </div>
  );
}
