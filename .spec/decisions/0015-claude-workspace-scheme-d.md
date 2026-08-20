# 0015 · Claude 工作台采用方案 D：三栏 + 分阶段，远端 CLI 走官方安装器

- 日期: 2026-08-20
- 状态: 生效

## 背景

桌面 Claude 工作台原先是左项目轨 + 右整页嵌入式 PTY，五个 stage tab（终端 / 文件 / 冲突 / 服务器状态 / 对话状态）切换中栏。原型实验 A/B/C/D 后，项目所有者拍板只做方案 D。

官方 Claude CLI 在 Linux 上的安装方式是 `curl -fsSL https://claude.ai/install.sh | bash`。`.spec/rules/system.md` 禁止的是 **agent 在本仓库 / 开发机上** 执行 sudo 与 curl|bash，不是产品通过 SSH 在用户自己的服务器上跑官方安装器。

## 决策

1. 产品模型三层：服务器（一个连接）→ 项目（这台机器上的一个文件夹）→ 会话（这个项目里的一个 Claude 对话）。连接状态、Claude 版本、Anthropic 登录是服务器级事实，只出现在服务器行。会话只出现在中栏标签条，左栏不列会话。
2. 界面：左 236px 服务器分组 → 项目；中 标签条 + 终端（期一是清单）；右 282px 文件浏览器；底栏 26px，点一段升起抽屉。五个 stage tab 退场，因为文件、冲突、服务器状态、对话状态分别落到右栏和底栏抽屉，不再打断正在进行的对话。
3. 三个阶段：期一初始化清单（连接 / 装同步组件 / 首次同步 / 装 Claude CLI / 登录 Anthropic）；期二日常对话；期三重新进来自动重连。
4. 远端安装官方 CLI 走官方安装器 `curl -fsSL https://claude.ai/install.sh | bash`，默认 latest 通道，装到 `~/.local/bin/claude`，禁止 sudo 与写入 `/usr/local/bin`。装完必须 `claude --version` 回读。agent 不得在开发机上执行该安装器。授权来源：项目所有者原话「应该是官方的Linux安装方式。curl -fsSL https://claude.ai/install.sh | bash」。

## 后果

- 旧规格「右工作台默认嵌入式 PTY / 五个 stage tab」作废，以方案 D 原型 `cc-home-d.html` / `cc-init.html` / `cc-resume.html` 为准。
- 安装失败必须按原因分码（无网 / DNS / 无 curl / 写不进 ~/.local/bin / 下载失败 / 校验失败），不能糊成一句「安装失败」。
- 仓库红线仍约束开发机操作；产品功能的官方安装器路径是已授权例外，reviewer 不得把这一条当违规退回。
