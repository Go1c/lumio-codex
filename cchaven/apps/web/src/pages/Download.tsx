import { Link } from "react-router-dom";

import { EmptyBlock, ErrorBlock, LoadingBlock } from "@/components/ui";
import { useT } from "@/i18n";
import { formatDate } from "@/lib/format";
import { usePublicConfig } from "@/state/publicConfig";

const APP_DEEP_LINK = "cchaven://open";

/** 4.3 下载页：版本号、下载地址、系统要求全部来自 `GET /config/public`。 */
export function Download() {
  const t = useT();
  const { status, data, error, reload } = usePublicConfig();

  if (status === "loading") {
    return (
      <div className="dl-hero">
        <div style={{ maxWidth: 520, margin: "0 auto" }}>
          <LoadingBlock lines={4} />
        </div>
      </div>
    );
  }

  if (status === "error" || !data) {
    return (
      <div className="dl-hero">
        <div style={{ maxWidth: 620, margin: "0 auto" }}>
          <ErrorBlock error={error} fallback={t("download.load_error")} onRetry={reload} />
        </div>
      </div>
    );
  }

  const releases = data.releases ?? [];
  const arm = releases.find((release) => release.arch === "arm64");
  const intel = releases.find((release) => release.arch === "x86_64");
  const current = arm ?? intel;

  return (
    <div className="dl-hero">
      <h1 style={{ fontSize: 36 }}>{t("download.title")}</h1>

      {current ? (
        <>
          <p className="section-sub" style={{ marginTop: 12 }}>
            {t("download.meta", {
              version: current.version,
              date: formatDate(current.released_at),
              minOS: current.min_os,
            })}
          </p>
          <div className="ctas" style={{ display: "flex", justifyContent: "center", gap: 14 }}>
            <a className="btn btn-primary btn-lg" href={(arm ?? current).download_url}>
              {t("download.cta_arm")}
            </a>
          </div>
          {intel && (
            <p style={{ marginTop: 12, fontSize: 13.5 }}>
              <a href={intel.download_url}>{t("download.cta_intel")}</a>
            </p>
          )}
        </>
      ) : (
        <EmptyBlock icon="📦" text={t("download.empty")} />
      )}

      <div className="steps3">
        <div className="step">
          <div className="num" aria-hidden="true">
            1
          </div>
          {t("download.step1")}
        </div>
        <div className="step">
          <div className="num" aria-hidden="true">
            2
          </div>
          {t("download.step2")}
        </div>
        <div className="step">
          <div className="num" aria-hidden="true">
            3
          </div>
          {t("download.step3")}
        </div>
      </div>

      <p style={{ marginTop: 40, color: "var(--gray)", fontSize: 14 }}>
        {t("download.installed")}
        <a href={APP_DEEP_LINK}>{t("download.open_app")}</a>
        {" · "}
        <Link to="/signup">{t("signup.title")}</Link>
      </p>
    </div>
  );
}
