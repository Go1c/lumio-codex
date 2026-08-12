import { afterEach, describe, expect, it, vi } from "vitest";

import {
  CDN_LATEST_URL,
  GITHUB_RELEASES_URL,
  LOCAL_LATEST_URL,
  assetForPlatform,
  detectPlatform,
  loadReleaseManifest,
} from "@/lib/releases";

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
  it("按 UA 区分 Apple 芯片、Intel 与 Windows", () => {
    expect(
      detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15"),
    ).toBe("mac-intel");
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Mac OS X 14_0) AppleWebKit/605.1.15")).toBe(
      "mac-arm",
    );
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows");
  });
});

describe("assetForPlatform", () => {
  it("每个平台只认自己的安装包，Windows 取 setup 而不是 portable", () => {
    expect(assetForPlatform(MANIFEST, "mac-arm")?.url).toBe("https://s3.example.com/arm64.dmg");
    expect(assetForPlatform(MANIFEST, "mac-intel")?.url).toBe("https://s3.example.com/x64.dmg");
    expect(assetForPlatform(MANIFEST, "windows")?.url).toBe("https://s3.example.com/setup.exe");
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
