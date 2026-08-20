export type FileIconKind = "dir" | "dirOpen" | "file" | "fileText" | "fileCode" | "fileConf";

const FX_TEXT = new Set(["md", "txt", "yml", "yaml", "gitignore", "env", "log"]);
const FX_CODE = new Set(["ts", "tsx", "js", "jsx", "mjs", "rs", "py", "sh"]);
const FX_CONF = new Set(["json", "toml", "lock"]);

export function fileIconKind(path: string, kind: "dir" | "file", isOpen?: boolean): FileIconKind {
  if (kind === "dir") return isOpen ? "dirOpen" : "dir";
  const name = path.slice(path.lastIndexOf("/") + 1);
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  if (FX_CODE.has(ext)) return "fileCode";
  if (FX_CONF.has(ext)) return "fileConf";
  if (FX_TEXT.has(ext)) return "fileText";
  return "file";
}
