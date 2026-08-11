/**
 * Resolve-only ESM hook: map `.js` import specifiers → `.ts` sources and
 * `@fns/control-api` → package src. Pair with `--experimental-strip-types`.
 */
import { existsSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

function tryFile(p) {
  if (existsSync(p)) return p;
  return null;
}

function resolvePath(abs) {
  return (
    tryFile(abs) ||
    (abs.endsWith(".js") && tryFile(abs.slice(0, -3) + ".ts")) ||
    (abs.endsWith(".js") && tryFile(abs.slice(0, -3) + ".tsx")) ||
    (abs.endsWith(".jsx") && tryFile(abs.slice(0, -4) + ".tsx")) ||
    tryFile(abs + ".ts") ||
    tryFile(abs + ".tsx") ||
    tryFile(abs + ".js") ||
    tryFile(join(abs, "index.ts")) ||
    tryFile(join(abs, "index.tsx")) ||
    tryFile(join(abs, "index.js"))
  );
}

export async function resolve(specifier, context, nextResolve) {
  if (specifier === "@fns/control-api" || specifier.startsWith("@fns/control-api/")) {
    const pkgRoot = fileURLToPath(
      new URL("../../../packages/control-api/src/", import.meta.url),
    );
    const rest =
      specifier === "@fns/control-api"
        ? "index"
        : specifier.slice("@fns/control-api/".length).replace(/\.js$/, "");
    const hit = resolvePath(join(pkgRoot, rest));
    if (hit) return { shortCircuit: true, url: pathToFileURL(hit).href };
  }

  if (specifier.startsWith("./") || specifier.startsWith("../") || specifier.startsWith("/")) {
    const parent = context.parentURL ? fileURLToPath(context.parentURL) : process.cwd();
    const baseDir = extname(parent) ? dirname(parent) : parent;
    const abs = specifier.startsWith("/") ? specifier : join(baseDir, specifier);
    const hit = resolvePath(abs);
    if (hit) return { shortCircuit: true, url: pathToFileURL(hit).href };
  }

  return nextResolve(specifier, context);
}
