import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { HelpArticle, HelpIndex } from "../help/HelpCenter";
import { HELP_TOPICS } from "../help/topics";

function renderHelp(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/help" element={<HelpIndex />} />
        <Route path="/help/:slug" element={<HelpArticle />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("帮助中心", () => {
  it("至少覆盖安装、未签名、登录、修复、Claude 连服务器", () => {
    expect(HELP_TOPICS.map((topic) => topic.slug)).toEqual([
      "install",
      "unsigned",
      "login",
      "repair",
      "claude-server",
    ]);

    renderHelp("/help");

    expect(screen.getByRole("heading", { name: "需要什么帮助？" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /安装/ })).toHaveAttribute("href", "/help/install");
    expect(screen.getByRole("link", { name: /未签名/ })).toHaveAttribute("href", "/help/unsigned");
    expect(screen.getByRole("link", { name: /登录/ })).toHaveAttribute("href", "/help/login");
    expect(screen.getByRole("link", { name: /修复/ })).toHaveAttribute("href", "/help/repair");
    expect(screen.getByRole("link", { name: /Claude 连服务器/ })).toHaveAttribute(
      "href",
      "/help/claude-server",
    );
    expect(screen.getByText("https://bestcodex.app/help")).toBeInTheDocument();
  });

  it("单篇说明能渲染标题", () => {
    renderHelp("/help/login");
    expect(screen.getByRole("heading", { name: "登录" })).toBeInTheDocument();
  });

  it("未签名篇按 BestCodex.app 给出隔离标记解除命令", () => {
    renderHelp("/help/unsigned");

    expect(screen.getByRole("heading", { name: "未签名" })).toBeInTheDocument();
    expect(screen.getByText(/xattr -cr "\/Applications\/BestCodex.app"/)).toBeInTheDocument();
    expect(screen.queryByText(/Lumio Codex\.app/)).not.toBeInTheDocument();
  });
});
