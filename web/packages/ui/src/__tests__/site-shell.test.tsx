import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { SiteShell } from "../components/SiteShell";
import { portalAccountLinks } from "../config";

function renderShell(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

describe("SiteShell", () => {
  it("渲染品牌、导航与三站互链的页脚", () => {
    renderShell(
      <SiteShell
        brand={{ name: "CC避风港", nameEn: "CCHaven" }}
        site="cc"
        nav={[
          { label: "定价", href: "/pricing" },
          { label: "下载", href: "/download" },
        ]}
        accountLinks={portalAccountLinks("https://cc.lumiogame.com/")}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.getByRole("link", { name: /CC避风港/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "定价" })).toHaveAttribute("href", "/pricing");
    expect(screen.getByRole("link", { name: "Lumio Codex" })).toHaveAttribute(
      "href",
      "https://codex.lumiogame.com",
    );
    expect(screen.getByText("内容")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "客服与反馈" })).toBeInTheDocument();
  });

  it("未登录时账号入口跳门户并带 next 回跳", () => {
    renderShell(
      <SiteShell
        brand={{ name: "CC避风港" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.lumiogame.com/pricing")}
        account={{ status: "anonymous" }}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.getByRole("link", { name: "登录" })).toHaveAttribute(
      "href",
      "https://lumiogame.com/login?next=https%3A%2F%2Fcc.lumiogame.com%2Fpricing",
    );
    expect(screen.getByRole("link", { name: "注册" })).toHaveAttribute(
      "href",
      "https://lumiogame.com/signup?next=https%3A%2F%2Fcc.lumiogame.com%2Fpricing",
    );
  });

  it("已登录时显示账户入口与邮箱首字母", () => {
    renderShell(
      <SiteShell
        brand={{ name: "CC避风港" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.lumiogame.com/")}
        account={{ status: "authenticated", email: "user@example.com" }}
      >
        <p>内容</p>
      </SiteShell>,
    );

    const account = screen.getByRole("link", { name: /账户/ });
    expect(account).toHaveAttribute(
      "href",
      "https://lumiogame.com/account?next=https%3A%2F%2Fcc.lumiogame.com%2F",
    );
    expect(screen.getByText("U")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "登录" })).not.toBeInTheDocument();
  });

  it("会话尚未确定时不闪现登录入口", () => {
    renderShell(
      <SiteShell
        brand={{ name: "CC避风港" }}
        site="cc"
        accountLinks={portalAccountLinks("https://cc.lumiogame.com/")}
        account={{ status: "loading" }}
      >
        <p>内容</p>
      </SiteShell>,
    );

    expect(screen.queryByRole("link", { name: "登录" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /账户/ })).not.toBeInTheDocument();
  });
});
