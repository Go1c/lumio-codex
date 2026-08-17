import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "@/App";

const MANIFEST = {
  version: "1.2.46",
  assets: [
    {
      name: "LumioCodex-1.2.46-macos-arm64-internal-unsigned.dmg",
      url: "https://s3.example.com/arm64.dmg",
    },
    {
      name: "LumioCodex-1.2.46-windows-x64-setup-internal-unsigned.exe",
      url: "https://s3.example.com/setup.exe",
    },
  ],
};

function renderApp(path = "/") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Codex 产品站", () => {
  it("首页讲清「不是官方应用」的定位", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("更快开始使用官方 Codex");
    expect(screen.getByText(/你使用的始终是官方 Codex 应用/)).toBeInTheDocument();
  });

  it("带 Intel 字样的 Mac UA 把「你的设备」标在 Apple 芯片卡上", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );
    vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue(
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
    );

    renderApp();

    expect(
      within(screen.getByRole("article", { name: "macOS · Apple 芯片" })).getByText("你的设备"),
    ).toBeInTheDocument();
    expect(
      within(screen.getByRole("article", { name: "macOS · Intel" })).queryByText("你的设备"),
    ).not.toBeInTheDocument();
  });

  it("按平台给出安装包，并在下载前弹确认层", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(MANIFEST), { status: 200 }))),
    );

    renderApp();

    const card = screen.getByRole("article", { name: "macOS · Apple 芯片" });
    await waitFor(() => expect(within(card).getByText(/v1.2.46/)).toBeInTheDocument());

    await userEvent.click(within(card).getByRole("button", { name: "下载" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("link", { name: "开始下载" })).toHaveAttribute(
      "href",
      "https://s3.example.com/arm64.dmg",
    );
    expect(within(dialog).getByText(/xattr -cr/)).toBeInTheDocument();
  });

  it("清单里没有该平台的包时指向 GitHub 发布页", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(new Response(JSON.stringify({ version: "1.2.46", assets: [] }), { status: 200 })),
      ),
    );

    renderApp();

    const card = screen.getByRole("article", { name: "Windows · x64" });
    await waitFor(() => expect(within(card).getByText(/GitHub/)).toBeInTheDocument());

    await userEvent.click(within(card).getByRole("button", { name: "下载" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("link", { name: "前往发布页" })).toHaveAttribute(
      "href",
      "https://github.com/Go1c/lumio-codex/releases",
    );
  });

  it("CDN 不可用时三张卡都回退 GitHub，不留死链", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    await waitFor(() =>
      expect(screen.getAllByText(/CDN 暂不可用/).length).toBeGreaterThanOrEqual(3),
    );
  });

  it("页脚链到隐私政策与服务条款，并保留 AGPL 说明", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("link", { name: "隐私政策" })).toHaveAttribute("href", "/privacy");
    expect(screen.getByRole("link", { name: "服务条款" })).toHaveAttribute("href", "/terms");
    expect(screen.getByText(/AGPL-3.0-only/)).toBeInTheDocument();
  });

  it.each(["/privacy", "/terms"] as const)("%s 披露本机改写与不捆绑官方应用", (path) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp(path);

    expect(screen.getByText(/config\.toml/)).toBeInTheDocument();
    expect(screen.getAllByText(/auth\.json/).length).toBeGreaterThan(0);
    expect(screen.getByText(/api\.lumio\.games/)).toBeInTheDocument();
    expect(screen.getAllByText(/不捆绑/).length).toBeGreaterThan(0);
    expect(screen.getByText(/不修改官方应用/)).toBeInTheDocument();
  });

  it("账号入口回门户并带 next", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new Error("offline"))),
    );

    renderApp();

    expect(screen.getByRole("link", { name: "登录" }).getAttribute("href")).toMatch(
      /^https:\/\/lumiogame\.com\/login\?next=/,
    );
  });
});
