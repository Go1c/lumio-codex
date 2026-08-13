import { Link } from "react-router-dom";

import { PlanCard } from "@/components/PlanCard";
import { useT } from "@/i18n";
import { useInviteAttribution } from "@/state/inviteAttribution";

const TERMINAL_SHOT = [
  "$ ssh dev-server",
  "Attached to session cc-my-project",
  "",
  "> claude",
  "╭──────────────────────────────────────────╮",
  "│  Claude Code · my-project                │",
  "│  How can I help you today?               │",
  "╰──────────────────────────────────────────╯",
  "",
  "> Refactor the sync engine to batch writes_",
];

/** 4.1 首页（精简版）：Hero / 两列价值点 / 邀请横幅 / 定价摘要 + 下载 CTA。 */
export function Home() {
  const t = useT();
  const attribution = useInviteAttribution();
  // 归因未确定（加载中或接口失败）时按「无邀请」渲染常驻横幅，不会先高亮再回退。
  const invited = attribution?.attributed ? attribution : null;

  return (
    <>
      <section className="hero">
        <h1>
          {t("home.title")}
          <br />
          {t("home.title2")}
        </h1>
        <p className="sub">{t("home.subtitle")}</p>
        <div className="ctas">
          <Link to="/download" className="btn btn-primary btn-lg">
            {t("home.cta_download")}
          </Link>
          <Link to="/pricing" className="btn btn-secondary btn-lg">
            {t("home.cta_pricing")}
          </Link>
        </div>
        <div className="shot">
          <div className="shot-body" aria-label="CC避风港 终端界面截图">
            {TERMINAL_SHOT.map((line, index) => (
              <div key={index}>{line || "\u00A0"}</div>
            ))}
          </div>
        </div>
      </section>

      <section className="value-cols" aria-label="核心价值">
        <div className="card">
          <div className="icon" aria-hidden="true">
            🛡️
          </div>
          <h3>{t("home.value1_title")}</h3>
          <p>{t("home.value1_body")}</p>
        </div>
        <div className="card">
          <div className="icon" aria-hidden="true">
            🔄
          </div>
          <h3>{t("home.value2_title")}</h3>
          <p>{t("home.value2_body")}</p>
        </div>
      </section>

      <div className={`banner ok invite-strip ${invited ? "highlight" : ""}`.trim()}>
        <span>
          {invited
            ? t("home.invite_banner_active", { inviter: invited.inviter || "朋友" })
            : t("home.invite_banner")}
        </span>
      </div>

      <h2 className="section-title">{t("home.pricing_title")}</h2>
      <p className="section-sub">{t("home.pricing_sub")}</p>
      <PlanCard />

      <div className="ctas bottom-cta">
        <Link to="/download" className="btn btn-primary btn-lg">
          {t("home.cta_download")}
        </Link>
      </div>
    </>
  );
}
