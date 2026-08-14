/** 首屏底部的滚动提示：一句引导 + 呼吸下沉的箭头。 */
export function ScrollHint({ label }: { label: string }) {
  return (
    <div className="scroll-hint" aria-hidden="true">
      <span>{label}</span>
      <span className="chevron">⌄</span>
    </div>
  );
}
