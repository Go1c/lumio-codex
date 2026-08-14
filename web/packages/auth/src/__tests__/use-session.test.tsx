import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession, serializeCookie, writeSession } from "../session";
import { useSession } from "../useSession";

const fetchMock = vi.fn();

function Probe() {
  const session = useSession();
  return (
    <div>
      <span data-testid="status">{session.status}</span>
      <span data-testid="email">{session.profile?.email ?? "-"}</span>
      <span data-testid="token">{session.accessToken ?? "-"}</span>
    </div>
  );
}

/** 只留 refresh cookie：模拟 access cookie 到期被浏览器删除后的状态（W-1 主场景）。 */
function writeRefreshOnly(refreshToken: string): void {
  document.cookie = serializeCookie("lumio_rt", refreshToken, { maxAge: 3600 });
}

function envelope(data: unknown): Response {
  return new Response(JSON.stringify({ code: 0, data }), { status: 200 });
}

function rejected(reason: string): Response {
  return new Response(JSON.stringify({ code: 401, reason }), { status: 401 });
}

function profileBody(): Response {
  return envelope({ id: 1, email: "user@example.com", balance: 8, status: "active" });
}

function refreshedPair(): Response {
  return envelope({ access_token: "at-2", refresh_token: "rt-2", expires_in: 3600 });
}

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("useSession", () => {
  it("没有会话 Cookie 时立刻判定未登录，不请求接口", () => {
    render(<Probe />);

    expect(screen.getByTestId("status")).toHaveTextContent("anonymous");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("有会话 Cookie 时先按已登录渲染，再补上账户资料", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    fetchMock.mockResolvedValue(profileBody());

    render(<Probe />);

    expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    await waitFor(() => expect(screen.getByTestId("email")).toHaveTextContent("user@example.com"));
  });

  it("令牌已失效时清掉会话并退回未登录", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    fetchMock.mockResolvedValue(rejected("TOKEN_EXPIRED"));

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId("status")).toHaveTextContent("anonymous"));
    expect(document.cookie).not.toContain("lumio_at");
  });

  it("access cookie 过期后仅凭 refresh cookie 自动续期并恢复登录", async () => {
    writeRefreshOnly("rt-1");
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/api/v1/auth/refresh")) return refreshedPair();
      if (url.includes("/api/v1/auth/me")) return profileBody();
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId("email")).toHaveTextContent("user@example.com"));
    expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    expect(screen.getByTestId("token")).toHaveTextContent("at-2");
    expect(document.cookie).toContain("lumio_at=at-2");
    expect(document.cookie).toContain("lumio_rt=rt-2");
    const refreshCall = fetchMock.mock.calls.find((call) =>
      String(call[0]).includes("/api/v1/auth/refresh"),
    );
    expect(String(fetchMock.mock.calls[0]?.[1]?.body)).toContain('"refresh_token":"rt-1"');
    expect(refreshCall).toBeTruthy();
  });

  it("仅剩 refresh cookie 且续期失败时清掉会话退回未登录", async () => {
    writeRefreshOnly("rt-1");
    fetchMock.mockResolvedValue(rejected("REFRESH_TOKEN_REUSED"));

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId("status")).toHaveTextContent("anonymous"));
    expect(document.cookie).not.toContain("lumio_rt");
  });

  it("access 被服务端拒绝时刷新一次成功则恢复登录", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    let meCalls = 0;
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/api/v1/auth/refresh")) return refreshedPair();
      if (url.includes("/api/v1/auth/me")) {
        meCalls += 1;
        return meCalls === 1 ? rejected("TOKEN_EXPIRED") : profileBody();
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId("email")).toHaveTextContent("user@example.com"));
    expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    expect(screen.getByTestId("token")).toHaveTextContent("at-2");
  });
});
