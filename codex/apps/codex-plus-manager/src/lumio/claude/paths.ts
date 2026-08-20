export function projectSlug(name: string): string {
  let slug = "";
  let lastDash = false;
  for (const ch of name.trim()) {
    if (/[A-Za-z0-9._-]/.test(ch)) {
      slug += ch;
      lastDash = false;
    } else if (ch.trim() === "") {
      if (!lastDash) {
        slug += "-";
        lastDash = true;
      }
    } else if (ch.charCodeAt(0) > 127) {
      slug += ch;
      lastDash = false;
    } else if (!lastDash) {
      slug += "-";
      lastDash = true;
    }
  }
  slug = slug.replace(/^[-.]+|[-.]+$/g, "");
  return slug === "" ? "my-project" : slug;
}

export function remoteProjectRoot(_user: string, name: string): string {
  return `~/bestcodex/${projectSlug(name)}`;
}

export function localProjectRoot(name: string): string {
  return `~/BestCodex/${projectSlug(name)}`;
}
