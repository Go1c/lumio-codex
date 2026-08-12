import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ProjectWizard } from "./ProjectWizard";
import { renderWithProviders } from "../test/render";
import type { MockOptions } from "../lib/mockApi";

function setup(options: MockOptions = {}) {
  const onCancel = vi.fn();
  const onCompleted = vi.fn();
  const harness = renderWithProviders(
    <ProjectWizard onCancel={onCancel} onCompleted={onCompleted} />,
    options,
  );
  return { ...harness, onCancel, onCompleted };
}

async function fillStepOne(harness: ReturnType<typeof setup>) {
  await harness.user.type(screen.getByLabelText("服务器 IP 地址"), "43.156.20.8");
  await harness.user.type(screen.getByLabelText("密码"), "hunter2");
  await harness.user.click(screen.getByRole("button", { name: "连接并继续" }));
}

describe("新建项目向导（5.3）", () => {
  it("第 1 步提供帮助框和三个字段，密码明确说明只存在本机钥匙串", () => {
    setup();

    expect(screen.getByText(/在阿里云、腾讯云、AWS 等平台买好服务器后/)).toBeInTheDocument();
    expect(screen.getByLabelText("服务器 IP 地址")).toBeInTheDocument();
    expect(screen.getByLabelText("用户名")).toHaveValue("root");
    expect(screen.getByText("密码只保存在你自己的电脑上（系统钥匙串加密）。")).toBeInTheDocument();
  });

  it("粘贴整条 ssh 命令时自动拆填地址、用户名与端口，并提示已识别", async () => {
    const harness = setup();

    const address = screen.getByLabelText("服务器 IP 地址");
    await harness.user.click(address);
    await harness.user.paste("ssh ubuntu@43.156.20.8 -p 2222");

    await waitFor(() => expect(address).toHaveValue("43.156.20.8"));
    expect(screen.getByLabelText("用户名")).toHaveValue("ubuntu");
    expect(await screen.findByText("已自动识别服务器地址和用户名。")).toBeInTheDocument();

    await harness.user.click(screen.getByText("高级选项（懂 SSH 的用户使用）"));
    expect(screen.getByLabelText("端口")).toHaveValue("2222");
  });

  it("连接成功后自动进入第 2 步，目录按登录用户自动预设", async () => {
    const harness = setup();
    await fillStepOne(harness);

    expect(await screen.findByLabelText("项目名称")).toBeInTheDocument();
    await harness.user.type(screen.getByLabelText("项目名称"), "my-project");

    await waitFor(() =>
      expect(screen.getByLabelText("服务器上的项目目录")).toHaveTextContent(
        "/root/cchaven/my-project",
      ),
    );
    expect(screen.getByLabelText("电脑上的同步文件夹")).toHaveTextContent(
      "/Users/mary/CCHaven/my-project",
    );
    expect(screen.getAllByText("已自动设置")).toHaveLength(2);
  });

  it("预设目录点「修改」才可编辑，「用推荐值」可一键还原", async () => {
    const harness = setup();
    await fillStepOne(harness);
    await harness.user.type(await screen.findByLabelText("项目名称"), "api");

    const remoteField = screen.getByLabelText("服务器上的项目目录").closest(".field")!;
    await harness.user.click(within(remoteField as HTMLElement).getByRole("button", { name: "修改" }));

    const input = screen.getByLabelText("服务器上的项目目录");
    expect(input.tagName).toBe("INPUT");
    await harness.user.clear(input);
    await harness.user.type(input, "/srv/api");
    expect(input).toHaveValue("/srv/api");

    await harness.user.click(screen.getByRole("button", { name: "用推荐值" }));
    await waitFor(() =>
      expect(screen.getByLabelText("服务器上的项目目录")).toHaveTextContent("/root/cchaven/api"),
    );
  });

  it("排除规则预设四条，并固定提示机密文件永不同步且没有开关", async () => {
    const harness = setup();
    await fillStepOne(harness);
    await harness.user.type(await screen.findByLabelText("项目名称"), "api");

    await harness.user.click(screen.getByText("高级选项：同步排除规则"));
    expect(screen.getByLabelText(/不同步的内容/)).toHaveValue(
      ".git/\nnode_modules/\ntarget/\n.env",
    );
    expect(screen.getByText("🛡 机密文件（.env、密钥）默认受保护，永不同步。")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("完成设置跑完四个阶段并回调上层", async () => {
    const harness = setup();
    await fillStepOne(harness);
    await harness.user.type(await screen.findByLabelText("项目名称"), "my-project");
    await harness.user.click(screen.getByRole("button", { name: "下一步" }));

    expect(await screen.findByText("✓ 已验证连接")).toBeInTheDocument();
    await harness.user.click(screen.getByRole("button", { name: "完成设置" }));

    await waitFor(() => expect(harness.onCompleted).toHaveBeenCalledTimes(1));
    expect(screen.getByText("连接服务器")).toBeInTheDocument();
    expect(screen.getByText("安装CC避风港同步组件（自动完成，无需操作）")).toBeInTheDocument();
    expect(screen.getByText("创建项目目录 /root/cchaven/my-project")).toBeInTheDocument();
    expect(screen.getByText(/首次同步/)).toBeInTheDocument();

    // 阶段文案不出现 agent / tmux 术语。
    const stageText = screen.getByText("安装CC避风港同步组件（自动完成，无需操作）").textContent ?? "";
    expect(stageText.toLowerCase()).not.toContain("agent");
    expect(stageText.toLowerCase()).not.toContain("tmux");
  });

  it("部署失败停在该阶段并支持从失败处重试", async () => {
    const harness = setup({ failDeployAtStage: 1 });
    await fillStepOne(harness);
    await harness.user.type(await screen.findByLabelText("项目名称"), "my-project");
    await harness.user.click(screen.getByRole("button", { name: "下一步" }));
    await harness.user.click(screen.getByRole("button", { name: "完成设置" }));

    expect(
      await screen.findByText(
        "服务器磁盘空间不足（剩余 120 MB）。请清理磁盘后点击重试，或联系客服协助。",
      ),
    ).toBeInTheDocument();

    const retry = screen.getByRole("button", { name: "重试" });
    await harness.user.click(retry);
    // 重试从失败阶段（索引 1）续跑，而不是从头再来。
    expect(harness.api.calls).toContain("deployProject:1");
  });

  it("连接失败时给出按命中率排序的排查清单", async () => {
    const harness = setup({ failConnection: true });
    await fillStepOne(harness);

    expect(await screen.findByText("连不上服务器，请按顺序检查：")).toBeInTheDocument();
    const items = screen.getAllByRole("listitem").map((item) => item.textContent);
    // 服务端明确报了鉴权失败，密码这一条排到最前。
    expect(items[0]).toContain("密码是否正确");
    expect(items[1]).toContain("IP 地址");
    expect(items[2]).toContain("安全组");
  });

  it("Esc 关闭模态（部署未进行时）", async () => {
    const harness = setup();
    await harness.user.keyboard("{Escape}");
    expect(harness.onCancel).toHaveBeenCalled();
  });
});
