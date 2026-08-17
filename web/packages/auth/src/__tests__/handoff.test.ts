import { afterEach, describe, expect, it, vi } from "vitest";

import { clearSession, readSession } from "../session";
import {
  buildHandoffHash,
  consumeHandoff,
  parseHandoffHash,
  stripHandoffHash,
  withHandoff,
} from "../handoff";

const TOKENS = { accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 };

afterEach(() => {
  clearSession();
});

describe("parseHandoffHash / buildHandoffHash", () => {
  it("把令牌编进 hash 再读回来", () => {
    const hash = buildHandoffHash(TOKENS);
    expect(hash.startsWith("#")).toBe(true);
    expect(parseHandoffHash(hash)).toEqual(TOKENS);
  });

  it("没有完整三项时返回 null，不把半截令牌当会话", () => {
    expect(parseHandoffHash("#lumio_at=at-1")).toBeNull();
    expect(parseHandoffHash("#downloads")).toBeNull();
    expect(parseHandoffHash("")).toBeNull();
  });

  it("过期秒数非法时丢弃", () => {
    expect(parseHandoffHash("#lumio_at=at-1&lumio_rt=rt-1&lumio_exp=0")).toBeNull();
    expect(parseHandoffHash("#lumio_at=at-1&lumio_rt=rt-1&lumio_exp=abc")).toBeNull();
  });
});

describe("withHandoff", () => {
  it("只给官方入口追加 hash，外站原样返回以免泄露令牌", () => {
    expect(withHandoff("https://bestcodex.app/codex", TOKENS)).toContain("lumio_at=at-1");
    expect(withHandoff("https://lumiogame.com/account", TOKENS)).toContain("lumio_rt=rt-1");
    expect(withHandoff("https://evil.com/steal", TOKENS)).toBe("https://evil.com/steal");
  });

  it("保留原有 hash 片段（如下载锚点）", () => {
    const url = withHandoff("https://bestcodex.app/#downloads", TOKENS);
    expect(url).toContain("downloads");
    expect(url).toContain("lumio_at=at-1");
  });
});

describe("stripHandoffHash", () => {
  it("只剥交接参数，留下业务锚点", () => {
    expect(stripHandoffHash("#lumio_at=at-1&lumio_rt=rt-1&lumio_exp=3600&downloads")).toBe(
      "#downloads",
    );
    expect(stripHandoffHash(buildHandoffHash(TOKENS))).toBe("");
  });
});

describe("consumeHandoff", () => {
  it("官方主机上写入会话并 replaceState 抹掉 hash", () => {
    const replaceState = vi.fn();
    const location = {
      hash: buildHandoffHash(TOKENS),
      href: `https://bestcodex.app/codex${buildHandoffHash(TOKENS)}`,
      hostname: "bestcodex.app",
      pathname: "/codex",
      search: "",
    };

    expect(consumeHandoff(location, { replaceState })).toBe(true);
    expect(readSession()?.accessToken).toBe("at-1");
    expect(readSession()?.refreshToken).toBe("rt-1");
    expect(replaceState).toHaveBeenCalledOnce();
    const nextUrl = String(replaceState.mock.calls[0]?.[2]);
    expect(nextUrl).not.toContain("lumio_at=");
    expect(nextUrl).toContain("https://bestcodex.app/codex");
  });

  it("非官方主机不消费，避免把令牌写到陌生域", () => {
    const replaceState = vi.fn();
    const ok = consumeHandoff(
      {
        hash: buildHandoffHash(TOKENS),
        href: `https://evil.com/${buildHandoffHash(TOKENS)}`,
        hostname: "evil.com",
        pathname: "/",
        search: "",
      },
      { replaceState },
    );
    expect(ok).toBe(false);
    expect(readSession()).toBeNull();
    expect(replaceState).not.toHaveBeenCalled();
  });
});
