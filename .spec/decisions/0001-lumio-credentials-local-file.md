# 0001 · Lumio 凭据本期存 Lumio 自有数据目录的受限权限文件，不引入系统凭据库依赖

- 日期：2026-08-12
- 状态：生效

## 背景

`docs/specs/2026-08-11-lumio-codex-branded-client-design.md`「凭据与本地数据」要求把 Sub2API 的认证令牌与桌面 API Key 写入操作系统凭据库（macOS Keychain / Windows Credential Manager），并为两者约定了独立的 service / account 记录名。

落地时发现本仓库没有任何可用的凭据库依赖：workspace 未声明 `keyring`，macOS 侧无 `security-framework`，`windows` crate 也未启用 `Win32_Security_Credentials`。要真实访问系统凭据库必须新增依赖，而 `.spec/rules/system.md` 规定「不得擅自修改 `Cargo.toml`」，须先取得用户确认。

向用户提出三个选项（新增 `keyring` 依赖 / 本地受限权限文件 / 调系统 CLI），用户选择不新增依赖，凭据存本地文件。

## 决策

本期凭据落 Lumio 自有数据目录下的单个 JSON 文件，不引入任何新依赖：

- 位置由 `codex_plus_core::lumio::product::state_dir()` 派生，不复用 Codex++ Manager 数据目录。
- 写入走「临时文件 + 原子替换」，Unix 权限收紧为 `0o600`，Windows 依赖用户目录 ACL。
- 前端接口维持设计文档的约束不变：Tauri 命令只返回「存在 / 缺失 / 失效」状态，明文令牌与 API Key 只在后端进程内短暂持有，永不跨 IPC 边界。
- 存储后端封装在单一模块内，对外只暴露与凭据库无关的抽象接口，后续换成系统凭据库不改动上层账户逻辑。

## 后果

- **偏离已定稿设计**：与设计文档「操作系统凭据库是 Lumio 的秘密来源」直接冲突。该偏离必须在交付的 known gaps 中显式声明，不得当作已实现。
- **安全强度下降**：任何以当前用户身份运行的进程都能读取该文件；文件权限只防其他用户，不防同用户的其他程序。系统凭据库本可提供的进程级隔离与钥匙串访问控制在本期不存在。
- **换实现的成本已被限制**：因为存储后端被封装且对外接口不含明文，后续引入 `keyring` 只需替换该模块内部实现与其单元测试。
- 公开发布前应重新评估本决策；若届时仍未换成系统凭据库，需要在发布门槛中单列风险确认项。
