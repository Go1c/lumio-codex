export function ClaudeSubscribe({
  onOpenAccount,
  onBackToCodex,
}: {
  onOpenAccount: () => void;
  onBackToCodex: () => void;
}) {
  return (
    <main className="lumio-claude-onboard">
      <div className="lumio-claude-card is-subscribe">
        <span className="lumio-claude-icon" aria-hidden="true">
          <img alt="" src="/lumio-icon.png" />
        </span>
        <span className="lumio-claude-chip">Claude</span>
        <h2>
          在自己的服务器上
          <br />
          跑 Claude
        </h2>
        <p className="lumio-claude-price">
          ¥19.9<span> / 月</span>
        </p>
        <p>独立环境、双向同步、不限项目。用现在这个账号开通即可。</p>
        <button className="lumio-button is-primary is-large" onClick={onOpenAccount} type="button">
          开通 Claude
        </button>
        <p className="lumio-claude-quiet">
          <button className="lumio-link-button" onClick={onBackToCodex} type="button">
            回到 Codex Tab
          </button>
          ，先启动官方应用。
        </p>
      </div>
    </main>
  );
}
