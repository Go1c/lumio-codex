const URL_CHAR = /[A-Za-z0-9\-._~:/?#[\]@!$&'()*+,;=%]/;
const SENTENCE_AFTER_URL = /^(Browser|Paste|Login|Welcome|Tips|Esc|Use |and |or |see )/i;

export function stitchWrappedHttpsUrls(text: string): string[] {
  const found: string[] = [];
  const startRe = /https:\/\//g;
  let match: RegExpExecArray | null;
  let consumedThrough = 0;
  while ((match = startRe.exec(text)) !== null) {
    if (match.index < consumedThrough) continue;
    const consumed = consumeHttpsUrl(text, match.index);
    if (consumed.url.length > "https://".length) {
      found.push(trimTrailingUrlJunk(consumed.url));
      consumedThrough = consumed.end;
      startRe.lastIndex = consumed.end;
    }
  }
  return [...new Set(found)];
}

export function firstOpenableHttpsUrl(text: string): string | null {
  return stitchWrappedHttpsUrls(text)[0] ?? null;
}

export function textForClipboard(selection: string, visibleText: string): string | null {
  if (selection.trim()) {
    return firstOpenableHttpsUrl(selection) ?? selection;
  }
  return firstOpenableHttpsUrl(visibleText);
}

export function terminalContextActions(
  selection: string,
  visibleText: string,
): { copyText: string | null; openUrl: string | null } {
  const copyText = textForClipboard(selection, visibleText);
  const openUrl = firstOpenableHttpsUrl(selection) ?? firstOpenableHttpsUrl(visibleText);
  return { copyText, openUrl };
}

export function isClaudeLoginUrl(url: string): boolean {
  return /claude\.com\/(?:cai\/)?oauth/i.test(url);
}

export function looksLikeClaudeLoginPrompt(text: string): boolean {
  const collapsed = text.replace(/\s+/g, "");
  return /Paste code here/i.test(text) && /https:\/\/\S*claude\.com/i.test(collapsed);
}

export function copyTextForKey(input: {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  selection: string;
  visibleText: string;
}): string | null {
  const key = input.key.length === 1 ? input.key.toLowerCase() : input.key;
  if (key !== "c" || input.altKey) return null;

  const selected = input.selection;
  const fromScreen = textForClipboard(selected, input.visibleText);
  const isModC = input.metaKey || (input.ctrlKey && input.shiftKey);
  const isCtrlC = input.ctrlKey && !input.shiftKey && !input.metaKey;
  const isBareC = !input.metaKey && !input.ctrlKey;

  if (isModC) return fromScreen;
  if (isCtrlC) {
    if (selected.trim()) return firstOpenableHttpsUrl(selected) ?? selected;
    return looksLikeClaudeLoginPrompt(input.visibleText) ? fromScreen : null;
  }
  if (isBareC && (selected.trim() || looksLikeClaudeLoginPrompt(input.visibleText))) {
    return fromScreen;
  }
  return null;
}

export function visibleTextFromBuffer(lines: Array<string | undefined>): string {
  return lines.map((line) => line ?? "").join("\n");
}

function consumeHttpsUrl(text: string, start: number): { url: string; end: number } {
  let url = "";
  let i = start;
  while (i < text.length) {
    const ch = text[i] ?? "";
    if (ch === "\n" || ch === "\r" || ch === " ") {
      const next = skipWrap(text, i);
      if (next === i) break;
      const rest = text.slice(next);
      if (!canContinueUrl(url, rest)) break;
      i = next;
      continue;
    }
    if (!URL_CHAR.test(ch)) break;
    url += ch;
    i += 1;
  }
  return { url, end: i };
}

function skipWrap(text: string, index: number): number {
  let i = index;
  while (i < text.length && (text[i] === "\n" || text[i] === "\r" || text[i] === " ")) {
    i += 1;
  }
  return i;
}

function canContinueUrl(urlSoFar: string, remaining: string): boolean {
  if (!remaining || SENTENCE_AFTER_URL.test(remaining)) return false;
  const first = remaining[0] ?? "";
  if (/^[?&#/=%]/.test(first)) return true;
  if (/[?&=/%]$/.test(urlSoFar)) return true;
  if (URL_CHAR.test(first) && urlSoFar.length > 20 && !/^[A-Z][a-z]{2,}/.test(remaining)) {
    return true;
  }
  return false;
}

function trimTrailingUrlJunk(url: string): string {
  return url.replace(/[),.;]+$/g, "");
}
