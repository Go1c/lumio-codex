import { Aurora, EmptyBlock, portalAccountLinks } from "@lumio/ui";

import { DOWNLOAD_STEPS, ccDownloadLinks } from "@/content";

export function Download() {
  const links = ccDownloadLinks();
  const hasDownload = Boolean(links.arm ?? links.intel);

  return (
    <>
      <Aurora variant="claude" />
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

        {/* 桌面端尚未注册 cchaven:// scheme（spec-gaps B4），深链在这里只会报
            「找不到处理该协议的应用」；注册完成前不放「打开 APP」入口（QA W-17）。 */}
        <p style={{ marginTop: 40, color: "var(--gray)", fontSize: 14 }}>
          下载完成？安装后从「应用程序」或启动台打开 CC避风港。
          {" · "}
          <a href={portalAccountLinks(window.location.href).signup}>创建 Lumio 账号</a>
        </p>
      </div>
    </>
  );
}
