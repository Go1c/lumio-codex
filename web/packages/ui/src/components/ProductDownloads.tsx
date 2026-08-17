import { useEffect, useId, useState } from "react";

import {
  GITHUB_RELEASES_URL,
  PLATFORMS,
  assetForPlatform,
  canReadArchitecture,
  detectPlatform,
  loadReleaseManifest,
  resolveRecommendedPlatform,
  type PlatformId,
  type ReleaseManifest,
} from "../lib/releases";
import { isServerRender } from "../lib/ssr";
import { Modal } from "./Modal";
import { Reveal } from "./Reveal";

interface PendingDownload {
  url: string;
  label: string;
}

type ManifestState =
  /** 预渲染态：还没发起请求，别把 loading 文案烙进静态 HTML。 */
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; manifest: ReleaseManifest }
  | { status: "error" };

const EN_PLATFORMS: Record<PlatformId, { title: string; requirement: string }> = {
  "mac-arm": { title: "Mac · Apple silicon", requirement: "macOS 13 or later" },
  "mac-intel": { title: "Mac · Intel", requirement: "macOS 13 or later" },
  windows: { title: "Windows", requirement: "Windows 10 / 11 · 64-bit" },
};

/** 两站共用的一个安装包：Mac Apple / Mac Intel / Windows。 */
export function ProductDownloads({
  headingId,
  locale = "zh",
}: { headingId?: string; locale?: "zh" | "en" } = {}) {
  const [state, setState] = useState<ManifestState>(() =>
    isServerRender() ? { status: "idle" } : { status: "loading" },
  );
  const [pending, setPending] = useState<PendingDownload | null>(null);
  // 预渲染没有 navigator，也不该猜设备：静态 HTML 里不标「你的设备」。
  const [recommended, setRecommended] = useState<PlatformId | null>(() =>
    isServerRender() ? null : detectPlatform(navigator.userAgent),
  );
  const baseId = useId();
  const titleId = headingId ?? `${baseId}-title`;

  useEffect(() => {
    let cancelled = false;
    if (canReadArchitecture(navigator)) {
      void resolveRecommendedPlatform(navigator).then((platform) => {
        if (!cancelled) setRecommended(platform);
      });
    }
    loadReleaseManifest()
      .then((manifest) => {
        if (!cancelled) setState({ status: "ready", manifest });
      })
      .catch(() => {
        if (!cancelled) setState({ status: "error" });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <section className="mk-section" id="downloads" aria-labelledby={titleId}>
      <Reveal>
        <span className="section-kicker">Download</span>
        <h2 className="section-title" id={titleId}>
          {locale === "en" ? "Download" : "下载"}
        </h2>
      </Reveal>

      <Reveal delay={0.08}>
        <div className="dl-grid">
          {PLATFORMS.map((platform) => {
            const copy = locale === "en" ? EN_PLATFORMS[platform.id] : platform;
            return (
              <DownloadCard
                key={platform.id}
                id={`${baseId}-${platform.id}`}
                title={copy.title}
                requirement={copy.requirement}
                meta={metaFor(state, platform.id, locale)}
                recommended={platform.id === recommended}
                recommendedLabel={locale === "en" ? "Your device" : "你的设备"}
                downloadLabel={locale === "en" ? "Download" : "下载"}
                onDownload={() => setPending(targetFor(state, platform.id, locale))}
              />
            );
          })}
        </div>
      </Reveal>

      {pending && (
        <Modal
          title={locale === "en" ? "You're downloading a beta build" : "你正在下载内测版"}
          onClose={() => setPending(null)}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => setPending(null)}>
                {locale === "en" ? "Cancel" : "取消"}
              </button>
              <a
                className="btn btn-primary"
                href={pending.url}
                rel="noopener noreferrer"
                onClick={() => setPending(null)}
              >
                {pending.label}
              </a>
            </>
          }
        >
          {locale === "en" ? (
            <>
              These are unsigned internal builds. If macOS says the app is damaged, run{" "}
              <code>xattr -cr &quot;/Applications/BestCodex.app&quot;</code> and open it again — or
              Control-click the icon and choose Open. On Windows, verify the source if SmartScreen
              warns you, then continue.
            </>
          ) : (
            <>
              这是未签名的内部测试制品。macOS 若提示「已损坏」，在终端执行{" "}
              <code>xattr -cr &quot;/Applications/BestCodex.app&quot;</code>{" "}
              后再打开；也可对 App 右键 → 打开。Windows 出现 SmartScreen 时请核对后继续。
            </>
          )}
        </Modal>
      )}
    </section>
  );
}

function DownloadCard({
  id,
  title,
  requirement,
  meta,
  recommended,
  recommendedLabel,
  downloadLabel,
  onDownload,
}: {
  id: string;
  title: string;
  requirement: string;
  meta: string;
  recommended: boolean;
  recommendedLabel: string;
  downloadLabel: string;
  onDownload: () => void;
}) {
  return (
    <article
      className={`dl-card ${recommended ? "is-recommended" : ""}`.trim()}
      aria-labelledby={id}
    >
      {recommended && <span className="chip">{recommendedLabel}</span>}
      <h3 id={id}>{title}</h3>
      <p className="meta">{requirement}</p>
      <span className="meta">{meta}</span>
      <button
        type="button"
        className={recommended ? "btn btn-primary" : "btn btn-secondary"}
        onClick={onDownload}
      >
        {downloadLabel}
      </button>
    </article>
  );
}

function metaFor(state: ManifestState, platform: PlatformId, locale: "zh" | "en"): string {
  if (locale === "en") {
    if (state.status === "idle") return "Beta · unsigned";
    if (state.status === "loading") return "Reading the latest version…";
    if (state.status === "error") return "CDN unavailable · GitHub fallback";
    const asset = assetForPlatform(state.manifest, platform);
    if (!asset?.url) return "No build for this platform — use GitHub";
    return `v${state.manifest.version ?? "latest"} · CDN`;
  }
  if (state.status === "idle") return "内测版 · 未签名";
  if (state.status === "loading") return "读取最新版本…";
  if (state.status === "error") return "CDN 暂不可用 · GitHub 回退";
  const asset = assetForPlatform(state.manifest, platform);
  if (!asset?.url) return "暂无该平台包，请用 GitHub";
  return `v${state.manifest.version ?? "latest"} · CDN`;
}

function targetFor(
  state: ManifestState,
  platform: PlatformId,
  locale: "zh" | "en",
): PendingDownload {
  const asset = state.status === "ready" ? assetForPlatform(state.manifest, platform) : null;
  if (locale === "en") {
    return asset?.url
      ? { url: asset.url, label: "Download now" }
      : { url: GITHUB_RELEASES_URL, label: "Open the release page" };
  }
  return asset?.url
    ? { url: asset.url, label: "开始下载" }
    : { url: GITHUB_RELEASES_URL, label: "前往发布页" };
}
