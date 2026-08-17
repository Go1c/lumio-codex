import { LegalPage } from "@/pages/Legal";

export function Privacy() {
  return (
    <LegalPage title="隐私政策">
      <section className="acct-section">
        <h3>本机会改写哪些文件</h3>
        <p className="note">
          本产品会改写官方 Codex 用户目录里的 <code>~/.codex/config.toml</code> 和{" "}
          <code>~/.codex/auth.json</code>。若设置了 <code>$CODEX_HOME</code>，则写到那个目录。
        </p>
        <p className="note" style={{ marginTop: 12 }}>
          写入内容包括 <code>model</code>、<code>model_provider=lumio</code>、
          <code>[model_providers.lumio]</code>，以及 <code>auth.json</code> 里的{" "}
          <code>OPENAI_API_KEY</code>（Lumio 桌面 Key，不是官方 ChatGPT 登录态）。这样会把官方
          Codex 的请求指到远程中转 <code>https://api.lumio.games/v1</code>（Sub2API）。
        </p>
      </section>

      <section className="acct-section">
        <h3>不捆绑、不修改官方应用</h3>
        <p className="note">
          不捆绑、不下载、不修改、不注入官方 Codex / ChatGPT 应用本身，只检测并启动你已经安装的官方应用。
        </p>
      </section>

      <section className="acct-section">
        <h3>Lumio 账号存在哪</h3>
        <p className="note">
          Lumio 账号令牌存在本机自己的 <code>credentials.json</code>，不是系统钥匙串，也不是
          Windows Credential Manager。
        </p>
      </section>

      <section className="acct-section">
        <h3>退出登录不会自动恢复官方配置</h3>
        <p className="note">
          退出登录只删除 Lumio 凭据，不会自动恢复官方 Codex 配置。恢复是应用内的单独操作。
        </p>
      </section>

      <section className="acct-section">
        <h3>本机不做网络劫持</h3>
        <p className="note">本机不起代理、不改 hosts、不装证书。</p>
      </section>

      <section className="acct-section">
        <h3>本站收集什么</h3>
        <p className="note">
          bestcodex.app 是产品介绍站。注册、登录与充值在 Lumio 官网完成，本站不另建一套账号。运营主体与联系方式将补充。
        </p>
      </section>
    </LegalPage>
  );
}
