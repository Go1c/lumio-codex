import { EmptyBlock, portalAccountLinks } from "@lumio/ui";

import { DOWNLOAD_STEPS, ccDownloadLinks } from "@/content";

const APP_DEEP_LINK = "cchaven://open";

export function Download() {
  const links = ccDownloadLinks();
  const hasDownload = Boolean(links.arm ?? links.intel);

  return (
    <div className="dl-hero">
      <h1 style={{ fontSize: 36 }}>下载 CC避风港 APP</h1>
      <p className="section-sub" style={{ marginTop: 12 }}>
        {links.version ? `版本 ${links.version} · ` : ""}需要 macOS 13 及以上
      </p>

      {hasDownload ? (
        <>
          {links.arm && (
            <div className="ctas" style={{ display: "flex", justifyContent: "center", gap: 14 }}>
              <a className="btn btn-primary btn-lg" href={links.arm}>
                下载 macOS 版（Apple Silicon）
              </a>
            </div>
          )}
          {links.intel && (
            <p style={{ marginTop: 12, fontSize: 13.5 }}>
              <a href={links.intel}>下载 Intel 版</a>
            </p>
          )}
        </>
      ) : (
        <EmptyBlock icon="📦" text="下载地址尚未配置，请稍后再来或联系客服获取安装包。" />
      )}

      <div className="steps3">
        {DOWNLOAD_STEPS.map((step, index) => (
          <div className="step" key={step}>
            <div className="num" aria-hidden="true">
              {index + 1}
            </div>
            {step}
          </div>
        ))}
      </div>

      <p style={{ marginTop: 40, color: "var(--gray)", fontSize: 14 }}>
        已下载？<a href={APP_DEEP_LINK}>打开 CC避风港 APP</a>
        {" · "}
        <a href={portalAccountLinks(window.location.href).signup}>创建 Lumio 账号</a>
      </p>
    </div>
  );
}
