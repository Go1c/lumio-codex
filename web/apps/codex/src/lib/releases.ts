/** 下载区的版本指针与选包逻辑。平台识别不信任 UA 里冻住的 Intel 字样。 */

export type PlatformId = "mac-arm" | "mac-intel" | "windows";

export interface ReleaseAsset {
  name: string;
  url: string;
}

export interface ReleaseManifest {
  version?: string;
  assets?: ReleaseAsset[];
}

/** 同源指针由部署时随站点一起发布；读不到再退 CDN，避开 S3 的 CORS。 */
export const LOCAL_LATEST_URL = "/latest-internal.json";
export const CDN_LATEST_URL = "https://s3.lumio.games/lumio-codex/releases/latest-internal.json";
export const GITHUB_RELEASES_URL = "https://github.com/Go1c/lumio-codex/releases";

const PLATFORM_MATCHERS: Record<PlatformId, RegExp> = {
  "mac-arm": /macos-arm64-internal-unsigned\.dmg$/i,
  "mac-intel": /macos-x64-internal-unsigned\.dmg$/i,
  // 只认安装器，不要 portable 压缩包。
  windows: /windows-x64-setup-internal-unsigned\.exe$/i,
};

export const PLATFORMS: Array<{ id: PlatformId; title: string }> = [
  { id: "mac-arm", title: "macOS · Apple 芯片" },
  { id: "mac-intel", title: "macOS · Intel" },
  { id: "windows", title: "Windows · x64" },
];

type NavigatorLike = {
  userAgent: string;
  userAgentData?: {
    getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string }>;
  };
};

export function detectPlatform(userAgent: string, architecture?: string): PlatformId {
  if (/Macintosh|Mac OS X/i.test(userAgent)) {
    // Safari / Chrome 把 "Intel Mac OS X" 冻在 UA 里，Apple 芯片机也会带这个词。
    if (architecture && /x86/i.test(architecture)) return "mac-intel";
    return "mac-arm";
  }
  return "windows";
}

export function canReadArchitecture(nav: NavigatorLike): boolean {
  return typeof nav.userAgentData?.getHighEntropyValues === "function";
}

export async function resolveRecommendedPlatform(nav: NavigatorLike): Promise<PlatformId> {
  let architecture: string | undefined;
  try {
    architecture = (await nav.userAgentData?.getHighEntropyValues?.(["architecture"]))
      ?.architecture;
  } catch {
    // 高熵 Client Hints 可能被拒，退回 UA 默认。
  }
  return detectPlatform(nav.userAgent, architecture);
}

export function assetForPlatform(
  manifest: ReleaseManifest | null | undefined,
  platform: PlatformId,
): ReleaseAsset | null {
  const matcher = PLATFORM_MATCHERS[platform];
  if (!matcher || !Array.isArray(manifest?.assets)) return null;
  return manifest.assets.find((asset) => matcher.test(asset.name ?? "")) ?? null;
}

export async function loadReleaseManifest(): Promise<ReleaseManifest> {
  for (const url of [LOCAL_LATEST_URL, CDN_LATEST_URL]) {
    try {
      const response = await fetch(url, { cache: "no-store" });
      if (!response.ok) continue;
      return (await response.json()) as ReleaseManifest;
    } catch {
      // 换下一个来源
    }
  }
  throw new Error("Unable to load latest-internal.json");
}
