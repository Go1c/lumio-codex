export interface SshTarget {
  host: string;
  user: string | null;
  port: number | null;
}

function splitUserHost(token: string): SshTarget {
  const at = token.indexOf("@");
  const user = at > 0 ? token.slice(0, at) : null;
  const rest = at > 0 ? token.slice(at + 1) : token;
  const colon = rest.lastIndexOf(":");
  if (colon > 0) {
    const maybeHost = rest.slice(0, colon);
    const maybePort = rest.slice(colon + 1);
    if (!maybeHost.includes(":") && /^\d+$/.test(maybePort)) {
      return { host: maybeHost, user, port: Number(maybePort) };
    }
  }
  return { host: rest, user, port: null };
}

function valid(target: SshTarget): SshTarget | null {
  const host = target.host.trim();
  const plausible =
    host !== "" &&
    !host.startsWith("-") &&
    [...host].every((ch) => /[A-Za-z0-9._:-]/.test(ch)) &&
    /[A-Za-z0-9]/.test(host);
  if (!plausible) return null;
  return { ...target, host };
}

export function parseSshTarget(text: string): SshTarget | null {
  const trimmed = text.trim();
  if (trimmed === "" || trimmed.includes("\n")) return null;

  const tokens = trimmed.split(/\s+/);
  let target: string | null = null;
  let port: number | null = null;

  if (tokens[0]?.toLowerCase() === "ssh") {
    for (let i = 1; i < tokens.length; i += 1) {
      const token = tokens[i];
      if (token === "-p") {
        const value = tokens[i + 1];
        if (value && /^\d+$/.test(value)) {
          port = Number(value);
          i += 1;
        }
      } else if (token.startsWith("-p") && token.length > 2 && /^\d+$/.test(token.slice(2))) {
        port = Number(token.slice(2));
      } else if (token === "-i" || token === "-o" || token === "-l" || token === "-J" || token === "-F") {
        i += 1;
      } else if (token.startsWith("-")) {
        continue;
      } else if (target === null) {
        target = token;
      } else {
        break;
      }
    }
  } else {
    target = trimmed;
  }

  if (target === null) return null;
  const parsed = splitUserHost(target);
  parsed.port = parsed.port ?? port;
  return valid(parsed);
}
