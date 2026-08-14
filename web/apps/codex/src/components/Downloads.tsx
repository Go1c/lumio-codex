import { useEffect, useId, useState } from "react";

import { Modal, Reveal } from "@lumio/ui";

import {
  GITHUB_RELEASES_URL,
  PLATFORMS,
  assetForPlatform,
  detectPlatform,
  loadReleaseManifest,
  type PlatformId,
  type ReleaseManifest,
} from "@/lib/releases";

interface PendingDownload {
  url: string;
  label: string;
}

type ManifestState =
  | { status: "loading" }
  | { status: "ready"; manifest: ReleaseManifest }
  | { status: "error" };

export function Downloads() {
  const [state, setState] = useState<ManifestState>({ status: "loading" });
  const [pending, setPending] = useState<PendingDownload | null>(null);
  const baseId = useId();
  const recommended = detectPlatform(navigator.userAgent);

  useEffect(() => {
    let cancelled = false;
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
    <section className="section" id="downloads" aria-labelledby={`${baseId}-title`}>
      <Reveal>
        <span className="section-kicker">Downloads</span>
        <h2 className="section-title" id={`${baseId}-title`}>
          下载
        </h2>
        <p className="section-sub">
          当前为内测渠道，制品未签名。从浏览器下载后 macOS 可能提示「已损坏」——那是隔离标记，不是真坏了。
        </p>
      </Reveal>

      <Reveal delay={0.08}>
        <div className="dl-grid">
          {PLATFORMS.map((platform) => (
            <DownloadCard
              key={platform.id}
              id={`${baseId}-${platform.id}`}
              title={platform.title}
              meta={metaFor(state, platform.id)}
              recommended={platform.id === recommended}
              onDownload={() => setPending(targetFor(state, platform.id))}
            />
          ))}
        </div>
      </Reveal>

      {pending && (
        <Modal
          title="你正在下载内测版"
          onClose={() => setPending(null)}
          footer={
            <>
              <button type="button" className="btn btn-secondary" onClick={() => setPending(null)}>
                取消
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
          这是未签名的内部测试制品。macOS 若提示「已损坏」，在终端执行{" "}
          <code>xattr -cr &quot;/Applications/Lumio Codex.app&quot;</code>{" "}
          后再打开；也可对 App 右键 → 打开。Windows 出现 SmartScreen 时请核对后继续。
        </Modal>
      )}
    </section>
  );
}

function DownloadCard({
  id,
  title,
  meta,
  recommended,
  onDownload,
}: {
  id: string;
  title: string;
  meta: string;
  recommended: boolean;
  onDownload: () => void;
}) {
  return (
    <article
      className={`dl-card ${recommended ? "is-recommended" : ""}`.trim()}
      aria-labelledby={id}
    >
      {recommended && <span className="chip">你的设备</span>}
      <h3 id={id}>{title}</h3>
      <span className="meta">{meta}</span>
      <button
        type="button"
        className={recommended ? "btn btn-primary" : "btn btn-secondary"}
        onClick={onDownload}
      >
        下载
      </button>
    </article>
  );
}

function metaFor(state: ManifestState, platform: PlatformId): string {
  if (state.status === "loading") return "读取最新版本…";
  if (state.status === "error") return "CDN 暂不可用 · GitHub 回退";
  const asset = assetForPlatform(state.manifest, platform);
  if (!asset?.url) return "暂无该平台包，请用 GitHub";
  return `v${state.manifest.version ?? "latest"} · CDN`;
}

function targetFor(state: ManifestState, platform: PlatformId): PendingDownload {
  const asset = state.status === "ready" ? assetForPlatform(state.manifest, platform) : null;
  return asset?.url
    ? { url: asset.url, label: "开始下载" }
    : { url: GITHUB_RELEASES_URL, label: "前往发布页" };
}
