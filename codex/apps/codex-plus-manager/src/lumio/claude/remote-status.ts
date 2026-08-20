export function serviceDisplayName(key: string): string {
  switch (key) {
    case "sync":
      return "同步组件";
    case "workspace":
      return "远端服务";
    case "claude":
      return "Claude";
    default:
      return key;
  }
}

export function formatStatusBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
  const gb = mb / 1024;
  return `${gb < 10 ? gb.toFixed(1) : Math.round(gb)} GB`;
}

export function formatCapturedClock(capturedAt: string): string {
  const asNumber = Number(capturedAt);
  const date = Number.isFinite(asNumber) && asNumber > 0 ? new Date(asNumber) : new Date(capturedAt);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString();
}
