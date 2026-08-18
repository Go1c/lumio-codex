import { afterEach, describe, expect, it, vi } from "vitest";

import {
  CDN_LATEST_URL,
  GITHUB_RELEASES_URL,
  LOCAL_LATEST_URL,
  assetForPlatform,
  canReadArchitecture,
  detectPlatform,
  loadReleaseManifest,
  resolveRecommendedPlatform,
} from "@/lib/releases";

const SAFARI_ON_APPLE_SILICON =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15";

const MANIFEST = {
  version: "1.2.46",
  assets: [
    {
      name: "LumioCodex-1.2.46-macos-arm64-internal-unsigned.dmg",
      url: "https://s3.example.com/arm64.dmg",
    },
    {
      name: "LumioCodex-1.2.46-macos-x64-internal-unsigned.dmg",
      url: "https://s3.example.com/x64.dmg",
    },
    {
      name: "LumioCodex-1.2.46-windows-x64-portable-internal-unsigned.zip",
      url: "https://s3.example.com/portable.zip",
    },
    {
      name: "LumioCodex-1.2.46-windows-x64-setup-internal-unsigned.exe",
      url: "https://s3.example.com/setup.exe",
    },
  ],
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("detectPlatform", () => {
  it("不把 UA 里冻住的 Intel 字样当成 Intel Mac", () => {
    expect(detectPlatform(SAFARI_ON_APPLE_SILICON)).toBe("mac-arm");
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Mac OS X 14_0) AppleWebKit/605.1.15")).toBe(
      "mac-arm",
    );
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows");
  });

  it("只有 Client Hints 明确给出 x86 时才标 Intel Mac", () => {
    expect(detectPlatform(SAFARI_ON_APPLE_SILICON, "x86")).toBe("mac-intel");
    expect(detectPlatform(SAFARI_ON_APPLE_SILICON, "arm")).toBe("mac-arm");
  });
});

describe("resolveRecommendedPlatform", () => {
  it("读到 UA-CH architecture 后纠正推荐平台", async () => {
    await expect(
      resolveRecommendedPlatform({
        userAgent: SAFARI_ON_APPLE_SILICON,
        userAgentData: {
          getHighEntropyValues: async () => ({ architecture: "x86" }),
        },
      }),
    ).resolves.toBe("mac-intel");
  });

  it("没有 Client Hints 时按 Apple 芯片推荐", async () => {
    await expect(resolveRecommendedPlatform({ userAgent: SAFARI_ON_APPLE_SILICON })).resolves.toBe(
      "mac-arm",
    );
  });

  it("只在浏览器能提供 architecture hints 时才去读", () => {
    expect(canReadArchitecture({ userAgent: SAFARI_ON_APPLE_SILICON })).toBe(false);
    expect(
      canReadArchitecture({
        userAgent: SAFARI_ON_APPLE_SILICON,
        userAgentData: { getHighEntropyValues: async () => ({ architecture: "arm" }) },
      }),
    ).toBe(true);
  });
});

describe("assetForPlatform", () => {
  it("每个平台只认自己的安装包，Windows 取 setup 而不是 portable", () => {
    expect(assetForPlatform(MANIFEST, "mac-arm")?.url).toBe("https://s3.example.com/arm64.dmg");
    expect(assetForPlatform(MANIFEST, "mac-intel")?.url).toBe("https://s3.example.com/x64.dmg");
    expect(assetForPlatform(MANIFEST, "windows")?.url).toBe("https://s3.example.com/setup.exe");
  });

  it("Windows 也认已签名的 setup.exe，并优先于 unsigned", () => {
    const signed = assetForPlatform(
      {
        version: "1.2.46",
        assets: [
          {
            name: "LumioCodex-1.2.46-windows-x64-portable.zip",
            url: "https://s3.example.com/portable-signed.zip",
          },
          {
            name: "LumioCodex-1.2.46-windows-x64-setup-internal-unsigned.exe",
            url: "https://s3.example.com/setup-unsigned.exe",
          },
          {
            name: "LumioCodex-1.2.46-windows-x64-setup.exe",
            url: "https://s3.example.com/setup-signed.exe",
          },
        ],
      },
      "windows",
    );
    expect(signed?.url).toBe("https://s3.example.com/setup-signed.exe");
  });

  it("清单缺该平台的包时返回 null，由调用方回退 GitHub", () => {
    expect(assetForPlatform({ version: "1.0.0", assets: [] }, "mac-arm")).toBeNull();
    expect(assetForPlatform(null, "mac-arm")).toBeNull();
  });
});

describe("loadReleaseManifest", () => {
  it("优先读同源指针，避开 S3 的 CORS", async () => {
    const fetchMock = vi.fn((_input: RequestInfo | URL) =>
      Promise.resolve(new Response(JSON.stringify(MANIFEST), { status: 200 })),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(loadReleaseManifest()).resolves.toMatchObject({ version: "1.2.46" });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(String(fetchMock.mock.calls[0][0])).toBe(LOCAL_LATEST_URL);
  });

  it("同源指针缺失时回退 CDN", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) =>
      String(input) === LOCAL_LATEST_URL
        ? Promise.resolve(new Response("", { status: 404 }))
        : Promise.resolve(new Response(JSON.stringify(MANIFEST), { status: 200 })),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(loadReleaseManifest()).resolves.toMatchObject({ version: "1.2.46" });
    expect(String(fetchMock.mock.calls[1][0])).toBe(CDN_LATEST_URL);
  });

  it("两个来源都不可用时抛错，页面据此退回 GitHub 发布页", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    await expect(loadReleaseManifest()).rejects.toThrow();
    expect(GITHUB_RELEASES_URL).toContain("github.com");
  });
});
