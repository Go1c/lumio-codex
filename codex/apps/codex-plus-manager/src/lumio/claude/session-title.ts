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

export function firstSubmittedLine(input: string): string | null {
  const line = (input.split(/\r?\n/, 1)[0] ?? "").trimEnd();
  if (line.trim() === "") return null;
  if (isSlashCommand(line)) return null;
  return line;
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
