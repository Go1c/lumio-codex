// @vitest-environment node
//
// 安装脚本是纯文本资产，没有类型检查也没有构建期校验。它引用的三样东西——版本指针
// 地址、产物命名后缀、应用包名——都在别处定义；任何一处改了而脚本没跟着改，
// 用户拿到的就是一条会失败的命令。这里把这三处钉在一起。

import { describe, expect, it } from "vitest";

import { CDN_LATEST_URL, PLATFORMS } from "@lumio/ui";

// `?raw` 让脚本原样进来，不必给这个包引入 node 类型。
import powershell from "../../public/install.ps1?raw";
import shell from "../../public/install.sh?raw";

describe("macOS 安装脚本", () => {
  it("是 POSIX sh 且开启了失败即停", () => {
    expect(shell.startsWith("#!/bin/sh")).toBe(true);
    expect(shell).toContain("set -eu");
  });

  it("读的是与站点同一个版本指针", () => {
    expect(shell).toContain(CDN_LATEST_URL);
  });

  it("两种芯片各有对应的产物后缀，与下载区的匹配规则同源", () => {
    // 站点用正则匹配 macos-arm64 / macos-x64，脚本必须用同一套后缀，否则会挑错包。
    expect(shell).toContain("macos-arm64");
    expect(shell).toContain("macos-x64");
    expect(shell).toContain("internal-unsigned");
  });

  it("Rosetta 下不会把 Apple 芯片当成 Intel", () => {
    expect(shell).toContain("sysctl.proc_translated");
  });

  it("校验 SHA256，且取不到校验和时中止而不是继续装", () => {
    expect(shell).toContain("SHA256SUMS.txt");
    expect(shell).toContain("shasum -a 256");
    expect(shell).toMatch(/取不到校验和/);
    expect(shell).toMatch(/校验和不匹配/);
  });

  it("装完清掉隔离标记——未签名包不清就打不开", () => {
    expect(shell).toContain("xattr -cr");
  });

  it("从不调用 sudo，只在权限不足时提示换安装目录", () => {
    // 提示文案里可以出现 sudo 这个词，命令位置不行。
    const invokesSudo = shell
      .split("\n")
      .some((line) => /^\s*(?:sudo|.*[|&;]\s*sudo)\s/.test(line));
    expect(invokesSudo).toBe(false);
  });

  it("应用包名与帮助中心让用户敲的路径一致", () => {
    expect(shell).toContain('APP_NAME="BestCodex.app"');
  });
});

describe("Windows 安装脚本", () => {
  it("读同一个版本指针，并只取安装器而不是便携包", () => {
    expect(powershell).toContain(CDN_LATEST_URL);
    expect(powershell).toContain("windows-x64-setup-internal-unsigned.exe");
    expect(powershell).not.toContain("portable");
  });

  it("校验 SHA256，且不匹配时中止", () => {
    expect(powershell).toContain("SHA256SUMS.txt");
    expect(powershell).toContain("Get-FileHash");
    expect(powershell).toMatch(/校验和不匹配/);
  });
});

describe("两个脚本口径一致", () => {
  it("都支持 dry-run，便于在不落盘的前提下核对解析结果", () => {
    expect(shell).toContain("BESTCODEX_DRY_RUN");
    expect(powershell).toContain("BESTCODEX_DRY_RUN");
  });

  it("都说明这是未签名内测包，不含糊", () => {
    expect(shell).toMatch(/未签名/);
    expect(powershell).toMatch(/未签名/);
  });

  it("覆盖下载区列出的全部平台", () => {
    // 下载区有三张卡（mac-arm / mac-intel / windows），脚本合起来必须都能装。
    expect(PLATFORMS.map((platform) => platform.id).sort()).toEqual([
      "mac-arm",
      "mac-intel",
      "windows",
    ]);
  });
});
