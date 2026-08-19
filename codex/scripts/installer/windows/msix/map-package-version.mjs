import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

export function mapPackageVersionToMsix(packageVersion) {
  const raw = String(packageVersion ?? "");
  const nums = [...raw.matchAll(/\d+/g)].map((match) => Number(match[0]));
  if (nums.length < 3) {
    throw new Error(
      `Cannot map PACKAGE_VERSION '${packageVersion}' to MSIX x.y.z.w`,
    );
  }
  const parts = [nums[0], nums[1], nums[2], nums[3] ?? 0];
  for (const part of parts) {
    if (!Number.isInteger(part) || part < 0 || part > 65535) {
      throw new Error(`MSIX version part ${part} is outside 0-65535`);
    }
  }
  return parts.join(".");
}

const invokedDirectly =
  Boolean(process.argv[1]) &&
  fileURLToPath(import.meta.url) === resolve(process.argv[1]);

if (invokedDirectly) {
  try {
    process.stdout.write(`${mapPackageVersionToMsix(process.argv[2] ?? "")}\n`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
