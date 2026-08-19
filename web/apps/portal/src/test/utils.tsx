import { render } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { vi } from "vitest";

import { App } from "@/App";

export function envelope(data: unknown, status = 200): Response {
  return new Response(JSON.stringify({ code: 0, message: "success", data }), { status });
}

/** CCHaven 控制面信封是 `{data}`，与 Sub2API 不同。 */
export function ccEnvelope(data: unknown, status = 200): Response {
  return new Response(JSON.stringify({ data }), { status });
}

export function failure(status: number, reason: string): Response {
  return new Response(JSON.stringify({ code: status, message: "服务端原文", reason }), { status });
}

export const PROFILE = { id: 7, email: "user@example.com", balance: 12.5, status: "active" };

export const TOKENS = {
  access_token: "at-1",
  refresh_token: "rt-1",
  expires_in: 3600,
  user: PROFILE,
};

type Handler = (init: RequestInit | undefined) => Response;

/** 按 URL 片段分发的 fetch 桩；未预设的请求直接失败，避免测试悄悄打到真实网络。 */
export function stubFetch(handlers: Record<string, Handler>) {
  const mock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const key = Object.keys(handlers)
      .filter((candidate) => url.includes(candidate))
      .sort((a, b) => b.length - a.length)[0];
    if (!key) return Promise.reject(new Error(`未预设的请求：${url}`));
    return Promise.resolve(handlers[key](init));
  });
  vi.stubGlobal("fetch", mock);
  return mock;
}

export function renderApp(path = "/") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}
