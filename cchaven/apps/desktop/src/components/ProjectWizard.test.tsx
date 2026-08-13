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

  /** 走到第 3 步并先取得只读的部署预览。 */
  async function reachPreview(harness: ReturnType<typeof setup>) {
    await fillStepOne(harness);
    await harness.user.type(await screen.findByLabelText("项目名称"), "my-project");
    await harness.user.click(screen.getByRole("button", { name: "下一步" }));
    expect(await screen.findByText("✓ 已验证连接")).toBeInTheDocument();
    await harness.user.click(screen.getByRole("button", { name: "预览部署计划" }));
    return screen.findByText("即将执行的操作（此刻尚未改动服务器）");
  }

  it("先给出只读预览，确认后才动服务器", async () => {
    const harness = setup();
    expect(await reachPreview(harness)).toBeInTheDocument();

    // 预览阶段绝不能已经写过服务器。
    expect(harness.api.calls).toContain("previewDeployment");
    expect(harness.api.calls.some((call) => call.startsWith("executeDeployment"))).toBe(
      false,
    );
    expect(screen.getByText("共 10 步，全程可取消；失败会自动回滚。")).toBeInTheDocument();
  });

  it("完成设置按既定顺序跑完十步并回调上层", async () => {
    const harness = setup();
    await reachPreview(harness);
    await harness.user.click(screen.getByRole("button", { name: "完成设置" }));

    await waitFor(() => expect(harness.onCompleted).toHaveBeenCalledTimes(1));

    // 凭据 → 存项目 → 写服务器 → 验证访问：顺序即安全性。
    const at = (prefix: string) =>
      harness.api.calls.findIndex((call) => call.startsWith(prefix));
    const [provision, save, execute, probe] = [
      at("provisionCredential"),
      at("saveProject"),
      at("executeDeployment"),
      at("probeWorkspaceAccess"),
    ];
    expect([provision, save, execute, probe].every((index) => index >= 0)).toBe(true);
    expect(provision).toBeLessThan(save);
    expect(save).toBeLessThan(execute);
    expect(execute).toBeLessThan(probe);

    expect(screen.getByText("检查服务器环境")).toBeInTheDocument();
    expect(screen.getByText("上传同步代理")).toBeInTheDocument();
    expect(screen.getByText("验证服务与同步代理")).toBeInTheDocument();

    // 步骤文案对普通用户可读，不出现 agent / tmux / systemd 术语。
    const steps = ["检查服务器环境", "上传同步代理", "验证服务与同步代理"];
    for (const step of steps) {
      const text = (screen.getByText(step).textContent ?? "").toLowerCase();
      expect(text).not.toContain("agent");
      expect(text).not.toContain("tmux");
      expect(text).not.toContain("systemd");
    }
  });

  it("某一步失败时回滚并说明原因", async () => {
    const harness = setup({ failDeployAtStage: 1 });
    await reachPreview(harness);
    await harness.user.click(screen.getByRole("button", { name: "完成设置" }));

    expect(
      await screen.findByText(/服务器磁盘空间不足。请清理磁盘后重试。/),
    ).toBeInTheDocument();
    // 回滚必须发生：半配置好的服务器比失败更糟。
    await waitFor(() => expect(harness.api.calls).toContain("cancelProvisioning"));
    expect(harness.onCompleted).not.toHaveBeenCalled();
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
