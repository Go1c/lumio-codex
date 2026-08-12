import "@testing-library/jest-dom/vitest";
import { afterAll, afterEach, beforeAll } from "vitest";
import { cleanup, configure } from "@testing-library/react";
import { server } from "../mocks/server";
import { resetMockState } from "../mocks/data";

// 首个用例要等 MSW 与模块加载完成，默认 1 秒的等待窗口偏紧；
// 等待是事件驱动的，机器空闲时并不会真的等这么久。
configure({ asyncUtilTimeout: 8000 });

beforeAll(() => server.listen({ onUnhandledRequest: "error" }));

afterEach(() => {
  cleanup();
  server.resetHandlers();
  resetMockState();
});

afterAll(() => server.close());
