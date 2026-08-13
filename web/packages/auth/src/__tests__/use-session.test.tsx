import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession, writeSession } from "../session";
import { useSession } from "../useSession";

const fetchMock = vi.fn();

function Probe() {
  const session = useSession();
  return (
    <div>
      <span data-testid="status">{session.status}</span>
      <span data-testid="email">{session.profile?.email ?? "-"}</span>
    </div>
  );
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
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          code: 0,
          data: { id: 1, email: "user@example.com", balance: 8, status: "active" },
        }),
        { status: 200 },
      ),
    );

    render(<Probe />);

    expect(screen.getByTestId("status")).toHaveTextContent("authenticated");
    await waitFor(() => expect(screen.getByTestId("email")).toHaveTextContent("user@example.com"));
  });

  it("令牌已失效时清掉会话并退回未登录", async () => {
    writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ code: 401, reason: "TOKEN_EXPIRED" }), { status: 401 }),
    );

    render(<Probe />);

    await waitFor(() => expect(screen.getByTestId("status")).toHaveTextContent("anonymous"));
    expect(document.cookie).not.toContain("lumio_at");
  });
});
