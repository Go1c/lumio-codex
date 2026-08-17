import { LegalPage } from "@/pages/Legal";

export function Terms() {
  return (
    <LegalPage title="服务条款">
      <section className="acct-section">
        <h3>这是什么</h3>
        <p className="note">
          Lumio Codex 是独立的辅助接入工具，帮你完成 Lumio 账号登录和本机连接配置，让你更快用上已经安装的官方
          Codex。你日常使用的始终是官方 Codex 应用。本产品与 OpenAI、Codex、ChatGPT 无从属关系。
        </p>
      </section>

      <section className="acct-section">
        <h3>本机配置（你需要知情并同意）</h3>
        <p className="note">
          使用本产品即表示你同意它改写官方 Codex 用户目录里的 <code>~/.codex/config.toml</code> 和{" "}
          <code>~/.codex/auth.json</code>（若设置了 <code>$CODEX_HOME</code> 则写那里），把官方 Codex
          的请求指到远程中转 <code>https://api.lumio.games/v1</code>（Sub2API）。写入包括{" "}
          <code>model</code>、<code>model_provider=lumio</code>、<code>[model_providers.lumio]</code>
          ，以及 <code>auth.json</code> 的 <code>OPENAI_API_KEY</code>（Lumio 桌面 Key，不是官方
          ChatGPT 登录态）。
        </p>
      </section>

      <section className="acct-section">
        <h3>不捆绑、不修改官方应用</h3>
        <p className="note">
          不捆绑、不下载、不修改、不注入官方 Codex / ChatGPT 应用本身，只检测并启动你已安装的官方应用。
        </p>
      </section>

      <section className="acct-section">
        <h3>账号、退出与恢复</h3>
        <p className="note">
          Lumio 账号令牌存在本机 <code>credentials.json</code>（不是钥匙串 / Credential
          Manager）。退出登录只删除 Lumio 凭据，不会自动恢复官方 Codex 配置；恢复是应用内单独操作。
        </p>
      </section>

      <section className="acct-section">
        <h3>本机网络</h3>
        <p className="note">本机不起代理、不改 hosts、不装证书。</p>
      </section>

      <section className="acct-section">
        <h3>开源与免责</h3>
        <p className="note">
          本软件以 AGPL-3.0-only 开源。官方 Codex 的可用性与账号政策由其各自所有者决定；本工具只做接入配置，不保证官方应用始终可用。运营主体、地址、联系邮箱与备案/ICP 号将补充。
        </p>
      </section>
    </LegalPage>
  );
}
