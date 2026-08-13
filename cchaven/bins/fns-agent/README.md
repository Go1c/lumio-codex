# fns-agent

运行在用户云主机上的同步 agent：打开 `fns-sync-core`、监视工作区、经 loopback
`workspace-sync-v2` 与桌面端会话对接。

## 构建

见仓库 [`docs/ops/03-build-package.md`](../../docs/ops/03-build-package.md)「Linux agent 交叉编译」。

```bash
# 本机（与 host 同架构）
cargo build --locked -p fns-agent --release

# 常见：给 Ubuntu x86_64 用户机
cargo build --locked -p fns-agent --release --target x86_64-unknown-linux-gnu
```

## 运行

```bash
./fns-agent run --config /path/to/agent.json
./fns-agent status --config /path/to/agent.json --json
./fns-agent diagnose --config /path/to/agent.json --json
```

配置 schema：`fns-agent-config/1`。token 在独立 `0600` 文件（`tokenFile`），
永不写进 JSON。endpoint 必须是 loopback `workspace-sync-v2` URL。

## 与桌面部署的关系

桌面向导自动上传 agent 仍属缺口（`apps/desktop/docs/spec-gaps.md` B2）。
正式上线阶段的分发方式见 [`docs/ops/`](../../docs/ops/README.md)。
