export const DEFAULT_SESSION_TITLE = "新对话";
export const SESSION_TITLE_WIDTH = 22;

const WIDE_CHAR =
  /[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFF60]/;

export function clipWidth(text: string, budget: number): string {
  let width = 0;
  let out = "";
  for (const ch of text) {
    width += WIDE_CHAR.test(ch) ? 2 : 1;
    if (width > budget) return `${out}…`;
    out += ch;
  }
  return out;
}

export function isSlashCommand(line: string): boolean {
  return line.trim().startsWith("/");
}

export function feedTitleInput(
  buffer: string,
  data: string,
): { buffer: string; submitted: string[] } {
  const submitted: string[] = [];
  let buf = buffer;
  let i = 0;
  while (i < data.length) {
    if (data.startsWith("\x1bOM", i)) {
      submitted.push(buf);
      buf = "";
      i += 3;
      continue;
    }
    if (data.startsWith("\x1b[13~", i)) {
      submitted.push(buf);
      buf = "";
      i += 5;
      continue;
    }
    const ch = data[i] ?? "";
    if (ch === "\r") {
      if (data[i + 1] === "\n") i += 1;
      submitted.push(buf);
      buf = "";
      i += 1;
      continue;
    }
    if (ch === "\n") {
      submitted.push(buf);
      buf = "";
      i += 1;
      continue;
    }
    if (ch === "\x1b") {
      i += 1;
      while (i < data.length) {
        const next = data[i] ?? "";
        i += 1;
        if (/[A-Za-z~]/.test(next)) break;
      }
      continue;
    }
    if (ch === "\u007f" || ch === "\b") {
      buf = buf.slice(0, -1);
      i += 1;
      continue;
    }
    if (ch >= " " || ch === "\t") buf += ch;
    i += 1;
  }
  return { buffer: buf, submitted };
}

export function firstSubmittedLine(input: string): string | null {
  const line = (input.split(/\r?\n/, 1)[0] ?? "").trimEnd();
  if (line.trim() === "") return null;
  if (isSlashCommand(line)) return null;
  if (isTerminalDeviceReply(line)) return null;
  return line;
}

function isTerminalDeviceReply(line: string): boolean {
  const text = line.trim();
  if (text === "") return false;
  if (/[\x00-\x1f\x7f]/.test(text)) return true;
  if (text.includes("[>") || text.includes("[?") || text.includes("[<")) return true;
  return /^\[[\?><=0-9;]*[A-Za-z](?:\[[\?><=0-9;]*[A-Za-z])*$/.test(text);
}

export function lockTitleFromInput(
  current: { title: string | null; titleLocked: boolean },
  submitted: string,
): { title: string | null; titleLocked: boolean } {
  if (current.titleLocked) return current;
  const line = firstSubmittedLine(submitted);
  if (line === null) return current;
  return { title: clipWidth(line, SESSION_TITLE_WIDTH), titleLocked: true };
}
