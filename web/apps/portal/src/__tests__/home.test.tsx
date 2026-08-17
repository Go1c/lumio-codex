import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearSession } from "@lumio/auth";
import { siteUrl } from "@lumio/ui";

import { renderApp, stubFetch } from "@/test/utils";

afterEach(() => {
  clearSession();
  vi.unstubAllGlobals();
});

describe("门户首页交叉文案", () => {
  it("品牌是 BestCodex，产品卡指向产品站，不再写两个旧产品", () => {
    stubFetch({});
    renderApp("/");

    expect(document.querySelector(".site-header .logo")?.textContent).toMatch(/BestCodex/);
    expect(screen.queryByRole("link", { name: /^Lumio$/ })).not.toBeInTheDocument();
    expect(screen.queryByText("Lumio Codex")).not.toBeInTheDocument();
    expect(screen.queryByText("CC避风港")).not.toBeInTheDocument();
    expect(screen.getAllByText(/一个启动器/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/一次下载/).length).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("link", { name: /BestCodex/ }).some(
        (link) => link.getAttribute("href") === siteUrl("codex"),
      ),
    ).toBe(true);
    expect(document.querySelector(".product-panel svg")).toBeNull();
    expect(document.querySelector(".panel-mark")).toBeNull();
  });
});
