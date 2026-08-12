import "@testing-library/jest-dom/vitest";

import { configure } from "@testing-library/react";
import { afterAll, afterEach, beforeAll } from "vitest";

import { resetDb } from "@/mocks/db";
import { server } from "@/mocks/server";

// 当前 Node 版本下 jsdom 的 window.localStorage 不可用，用内存实现顶上。
// 应用代码本身不再读写 localStorage（见 invite-banner 的防回归用例），这里只是给那条断言一个可监听的对象。
if (!window.localStorage) {
  const store = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, String(value)),
      removeItem: (key: string) => void store.delete(key),
      clear: () => store.clear(),
      key: (index: number) => [...store.keys()][index] ?? null,
      get length() {
        return store.size;
      },
    },
  });
}

// 本机 jsdom 下每次 fetch 往返约 1 秒，默认 1 秒的等待窗口太紧。
configure({ asyncUtilTimeout: 5000 });

// jsdom 未实现剪贴板，复制邀请链接 / 授权码时会用到。
if (!navigator.clipboard) {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: () => Promise.resolve() },
  });
}

beforeAll(() => {
  server.listen({ onUnhandledRequest: "error" });

  // jsdom 提供的 AbortSignal 与 Node 的 fetch 实现不同源，直接传给 fetch 会被
  // 「Expected signal to be an instance of AbortSignal」拒绝。测试里剥掉 signal 即可；
  // 浏览器中不存在这个问题，生产代码照常用 AbortController 取消在途请求。
  const currentFetch = globalThis.fetch;
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) =>
    currentFetch(input, init?.signal ? { ...init, signal: undefined } : init)) as typeof fetch;
});

afterEach(() => {
  server.resetHandlers();
  resetDb();
  window.localStorage.clear();
});

afterAll(() => server.close());
