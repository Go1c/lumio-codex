/**
 * Terminal clipboard bridge helpers.
 *
 * Remote Claude / tmux apps copy via OSC 52:
 *   ESC ] 52 ; <pc> ; <base64> BEL
 *   ESC ] 52 ; <pc> ; <base64> ESC \
 *
 * The desktop xterm must strip those sequences from the display stream and
 * write the decoded payload to the local system clipboard.
 */

const OSC_PREFIX = "\x1b]";
const BEL = "\x07";
const ST = "\x1b\\";

/** Max pending buffer while waiting for an OSC terminator (bytes as UTF-16 code units). */
const MAX_PENDING = 64 * 1024;

export type Osc52PushResult = {
  /** Bytes/text that should still be rendered by xterm. */
  display: string;
  /** Decoded clipboard payloads found in this chunk (may be empty). */
  copies: string[];
};

/**
 * Incremental OSC 52 filter for a PTY byte stream decoded as UTF-8 text.
 * Safe across chunk boundaries; incomplete sequences are held in `pending`.
 */
export class Osc52Filter {
  private pending = "";

  /**
   * Push a decoded UTF-8 chunk. Returns display text (OSC 52 removed) and any
   * clipboard strings decoded from complete OSC 52 sequences.
   */
  push(chunk: string): Osc52PushResult {
    if (!chunk && !this.pending) {
      return { display: "", copies: [] };
    }

    let input = this.pending + chunk;
    this.pending = "";
    let display = "";
    const copies: string[] = [];

    while (input.length > 0) {
      const oscAt = input.indexOf(OSC_PREFIX);
      if (oscAt < 0) {
        display += input;
        input = "";
        break;
      }

      // Emit everything before the OSC introducer.
      display += input.slice(0, oscAt);
      const fromOsc = input.slice(oscAt);

      // Need at least ESC ] to start parsing; incomplete prefix → pending.
      if (fromOsc.length < 2) {
        this.pending = fromOsc;
        input = "";
        break;
      }

      // Find terminator: BEL or ST (ESC \).
      const belAt = fromOsc.indexOf(BEL);
      const stAt = fromOsc.indexOf(ST);
      let endAt = -1;
      let termLen = 0;
      if (belAt >= 0 && (stAt < 0 || belAt < stAt)) {
        endAt = belAt;
        termLen = 1;
      } else if (stAt >= 0) {
        endAt = stAt;
        termLen = 2;
      }

      if (endAt < 0) {
        // Incomplete sequence — hold it unless it exceeds the safety cap.
        if (fromOsc.length > MAX_PENDING) {
          // Treat as ordinary text to avoid unbounded growth on malformed input.
          display += fromOsc.slice(0, MAX_PENDING);
          input = fromOsc.slice(MAX_PENDING);
          continue;
        }
        this.pending = fromOsc;
        input = "";
        break;
      }

      const body = fromOsc.slice(2, endAt); // after ESC ]
      const rest = fromOsc.slice(endAt + termLen);

      // OSC body: 52;c;<base64>  (or 52;p0;… etc.)
      if (body.startsWith("52;")) {
        const decoded = decodeOsc52Body(body);
        if (decoded !== null) {
          copies.push(decoded);
          // Swallow the sequence (do not render).
        } else {
          // Malformed OSC 52 — drop it still (do not flash base64 at the user).
        }
      } else {
        // Other OSC sequences (title, colors, …) pass through unchanged.
        display += fromOsc.slice(0, endAt + termLen);
      }

      input = rest;
    }

    return { display, copies };
  }

  /** Flush any held incomplete sequence as display text (e.g. on dispose). */
  flush(): string {
    const left = this.pending;
    this.pending = "";
    return left;
  }
}

/**
 * Decode an OSC body that begins with `52;`.
 * Returns null if the payload is missing or not valid base64/UTF-8 text.
 */
export function decodeOsc52Body(body: string): string | null {
  // body = "52;<Pc>;<Pd>"  where Pd is base64 (or "?" for query — ignore).
  if (!body.startsWith("52;")) return null;
  const after = body.slice(3); // after "52;"
  const semi = after.indexOf(";");
  if (semi < 0) return null;
  // const pc = after.slice(0, semi); // clipboard selection; usually "c"
  const pd = after.slice(semi + 1);
  if (!pd || pd === "?") return null;
  try {
    // atob expects standard base64; strip whitespace that some apps insert.
    const cleaned = pd.replace(/\s+/g, "");
    const binary = atob(cleaned);
    // Convert binary string → UTF-8
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i) & 0xff;
    }
    return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  } catch {
    return null;
  }
}

/**
 * Write text to the local system clipboard.
 * Prefer the async Clipboard API; fall back to a temporary textarea for
 * WKWebView environments that reject navigator.clipboard without a gesture.
 */
export async function writeLocalClipboard(text: string): Promise<void> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // fall through
    }
  }
  fallbackCopy(text);
}

/** Read text from the local system clipboard (paste into the remote PTY). */
export async function readLocalClipboard(): Promise<string> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.readText) {
    try {
      return await navigator.clipboard.readText();
    } catch {
      // fall through — caller may have no clipboard permission
    }
  }
  return "";
}

function fallbackCopy(text: string): void {
  if (typeof document === "undefined") return;
  const el = document.createElement("textarea");
  el.value = text;
  el.setAttribute("readonly", "");
  el.style.position = "fixed";
  el.style.left = "-9999px";
  document.body.appendChild(el);
  el.select();
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(el);
  }
}
