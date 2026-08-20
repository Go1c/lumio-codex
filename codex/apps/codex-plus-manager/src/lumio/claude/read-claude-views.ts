import { readdir, readFile } from "node:fs/promises";

const viewsDir = new URL("../views/claude/", import.meta.url);

export async function readAllClaudeViews(): Promise<string> {
  const names = (await readdir(viewsDir))
    .filter((name) => (name.endsWith(".tsx") || name.endsWith(".ts")) && !name.endsWith(".test.ts"))
    .sort();
  const parts = await Promise.all(names.map((name) => readFile(new URL(name, viewsDir), "utf8")));
  return parts.join("\n");
}

export async function readAllClaudeCss(): Promise<string> {
  const names = (await readdir(viewsDir)).filter((name) => name.endsWith(".css")).sort();
  const parts = await Promise.all(names.map((name) => readFile(new URL(name, viewsDir), "utf8")));
  return parts.join("\n");
}
