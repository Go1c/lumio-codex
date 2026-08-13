import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clearSession, writeSession } from "@lumio/auth";

import { PROFILE, envelope, renderApp, stubFetch } from "@/test/utils";

// 离开 SPA 的唯一出口，jsdom 不实现真实导航；桩掉它才能断言「跳到哪」与「没有跳」。
const navigation = vi.hoisted(() => ({ urls: [] as string[] }));
vi.mock("@/lib/redirect", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/redirect")>();
  return { ...actual, goExternal: (url: string) => void navigation.urls.push(url) };
});

const REDIRECT_URI = "http://127.0.0.1:53682/callback";

function authorizeQuery(overrides: Record<string, string> = {}): string {
  return new URLSearchParams({
    client_id: "cchaven-desktop",
    redirect_uri: REDIRECT_URI,
    scope: "profile workspace offline_access",
    code_challenge: "c".repeat(43),
    code_challenge_method: "S256",
    state: "st4te",
    ...overrides,
  }).toString();
}

const CONTEXT = {
  client_name: "CC避风港 macOS",
  scopes: [
    { id: "profile", label: "读取你的账号邮箱与订阅状态" },
    { id: "workspace", label: "代表你连接与同步你的工作区" },
    { id: "offline_access", label: "在你未打开浏览器时保持登录" },
  ],
  redirect_kind: "loopback",
  logged_in: true,
  email: "user@example.com",
};

/** 控制面的信封与 Sub2API 不同：成功 `{data}`，失败 `{error:{code,message}}`。 */
function controlData(data: unknown): Response {
  return new Response(JSON.stringify({ data }), { status: 200 });
}

function controlError(status: number, code: string, message: string): Response {
  return new Response(JSON.stringify({ error: { code, message } }), { status });
}

function signIn() {
  writeSession({ accessToken: "at-1", refreshToken: "rt-1", expiresIn: 3600 });
}

beforeEach(() => {
  navigation.urls.length = 0;
});

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("授权确认页", () => {
  it("未登录时展示申请方与权限，并把完整授权参数带进登录入口", async () => {
    stubFetch({
      "/oauth/authorize/context": () =>
        controlData({ ...CONTEXT, logged_in: false, email: undefined }),
    });

    const query = authorizeQuery();
    renderApp(`/authorize?${query}`);

    expect(await screen.findByText(/CC避风港 macOS/)).toBeInTheDocument();
    expect(screen.getByText("代表你连接与同步你的工作区")).toBeInTheDocument();

    const next = encodeURIComponent(`/authorize?${query}`);
    expect(screen.getByRole("link", { name: "去登录" })).toHaveAttribute(
      "href",
      `/login?next=${next}`,
    );
    expect(screen.getByRole("link", { name: "创建账号" })).toHaveAttribute(
      "href",
      `/signup?next=${next}`,
    );
  });

  it("已登录时展示当前账号，确认后带 Sub2API 令牌换码并跳回桌面端", async () => {
    signIn();
    const redirectTo = `${REDIRECT_URI}?code=the-code&state=st4te`;
    const fetchMock = stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/oauth/authorize/context": () => controlData(CONTEXT),
      "/oauth/authorize": () =>
        controlData({ code: "the-code", redirect_to: redirectTo, expires_in: 300 }),
    });

    renderApp(`/authorize?${authorizeQuery()}`);

    expect(await screen.findByText(/user@example\.com/)).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "同意授权" }));

    await waitFor(() => expect(navigation.urls).toEqual([redirectTo]));

    const approve = fetchMock.mock.calls.find(
      ([url, init]) => String(url).includes("/oauth/authorize?") && init?.method === "POST",
    );
    expect(approve, "应向控制面发起授权请求").toBeDefined();
    expect((approve?.[1]?.headers as Record<string, string>).Authorization).toBe("Bearer at-1");
    // 兜底：桌面端超时后允许手动粘贴授权码。
    expect(screen.getByText("the-code")).toBeInTheDocument();
  });

  it("拒绝授权时按契约回跳 error，不签发授权码", async () => {
    signIn();
    const fetchMock = stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/oauth/authorize/context": () => controlData(CONTEXT),
    });

    renderApp(`/authorize?${authorizeQuery()}`);
    await userEvent.click(await screen.findByRole("button", { name: "拒绝" }));

    await waitFor(() => expect(navigation.urls).toHaveLength(1));
    const url = new URL(navigation.urls[0]);
    expect(url.origin + url.pathname).toBe(REDIRECT_URI);
    expect(url.searchParams.get("error")).toBe("access_denied");
    expect(url.searchParams.get("state")).toBe("st4te");
    expect(
      fetchMock.mock.calls.some(([, init]) => init?.method === "POST"),
      "拒绝不得触发授权请求",
    ).toBe(false);
  });

  it("redirect_uri 不在白名单时直接判参数非法，不接触控制面", async () => {
    const fetchMock = stubFetch({});

    renderApp(`/authorize?${authorizeQuery({ redirect_uri: "https://evil.com/callback" })}`);

    expect(await screen.findByText(/回调地址/)).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(navigation.urls).toEqual([]);
  });

  it("控制面判定参数非法时展示服务端原因，不给出授权按钮", async () => {
    signIn();
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/oauth/authorize/context": () =>
        controlError(400, "invalid_request", "code_challenge_method 必须为 S256"),
    });

    renderApp(`/authorize?${authorizeQuery({ code_challenge_method: "plain" })}`);

    expect(await screen.findByText("code_challenge_method 必须为 S256")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "同意授权" })).not.toBeInTheDocument();
  });

  it("控制面返回越界的 redirect_to 时拦住跳转", async () => {
    signIn();
    stubFetch({
      "/auth/me": () => envelope(PROFILE),
      "/oauth/authorize/context": () => controlData(CONTEXT),
      "/oauth/authorize": () =>
        controlData({
          code: "the-code",
          redirect_to: "https://evil.com/callback?code=the-code",
          expires_in: 300,
        }),
    });

    renderApp(`/authorize?${authorizeQuery()}`);
    await userEvent.click(await screen.findByRole("button", { name: "同意授权" }));

    expect(await screen.findByText(/不受信任的回调地址/)).toBeInTheDocument();
    expect(navigation.urls).toEqual([]);
  });
});
