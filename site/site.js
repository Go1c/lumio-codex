const LOCAL_LATEST_URL = "./latest-internal.json";
const CDN_LATEST_URL =
  "https://s3.lumio.games/lumio-codex/releases/latest-internal.json";
const GITHUB_RELEASES_URL = "https://github.com/Go1c/lumio-codex/releases";

const PLATFORM_MATCHERS = {
  "mac-arm": /macos-arm64-internal-unsigned\.dmg$/i,
  "mac-intel": /macos-x64-internal-unsigned\.dmg$/i,
  windows: /windows-x64-setup-internal-unsigned\.exe$/i,
};

function detectPlatform() {
  const ua = navigator.userAgent;
  if (/Macintosh|Mac OS X/i.test(ua)) {
    return /Intel/i.test(ua) ? "mac-intel" : "mac-arm";
  }
  return "windows";
}

function openModal() {
  document.getElementById("dl-confirm")?.classList.add("is-open");
}

function closeModal() {
  document.getElementById("dl-confirm")?.classList.remove("is-open");
}

function assetForPlatform(manifest, platform) {
  const matcher = PLATFORM_MATCHERS[platform];
  if (!matcher || !Array.isArray(manifest?.assets)) return null;
  return manifest.assets.find((asset) => matcher.test(asset.name || "")) || null;
}

function setCardMeta(card, text) {
  const meta = card.querySelector("[data-release-meta]");
  if (meta) meta.textContent = text;
}

function setDownloadTarget(url, label) {
  const go = document.getElementById("dl-go");
  if (!go) return;
  go.setAttribute("href", url);
  go.textContent = label;
}

async function loadReleaseManifest() {
  // Prefer same-origin pointer (avoids S3 CORS). Fall back to CDN JSON.
  for (const url of [LOCAL_LATEST_URL, CDN_LATEST_URL]) {
    try {
      const response = await fetch(url, { cache: "no-store" });
      if (!response.ok) continue;
      return await response.json();
    } catch (_) {
      // try next source
    }
  }
  throw new Error("Unable to load latest-internal.json");
}

document.addEventListener("DOMContentLoaded", async () => {
  const platform = detectPlatform();
  let pendingUrl = GITHUB_RELEASES_URL;
  let pendingLabel = "前往发布页";

  document.querySelectorAll(".dl-card").forEach((card) => {
    const recommended = card.dataset.platform === platform;
    card.classList.toggle("is-recommended", recommended);
    const chip = card.querySelector("[data-rec-chip]");
    if (chip) chip.hidden = !recommended;
  });

  try {
    const manifest = await loadReleaseManifest();
    const version = manifest.version || "latest";

    document.querySelectorAll(".dl-card").forEach((card) => {
      const cardPlatform = card.dataset.platform;
      const asset = assetForPlatform(manifest, cardPlatform);
      const button = card.querySelector("[data-open-download]");
      if (!asset?.url || !button) {
        setCardMeta(card, "暂无该平台包，请用 GitHub");
        button?.addEventListener("click", () => {
          pendingUrl = GITHUB_RELEASES_URL;
          pendingLabel = "前往发布页";
          setDownloadTarget(pendingUrl, pendingLabel);
          openModal();
        });
        return;
      }

      setCardMeta(card, `v${version} · CDN`);
      button.addEventListener("click", () => {
        pendingUrl = asset.url;
        pendingLabel = "开始下载";
        setDownloadTarget(pendingUrl, pendingLabel);
        openModal();
      });
    });

    const recommended = assetForPlatform(manifest, platform);
    if (recommended?.url) {
      pendingUrl = recommended.url;
      pendingLabel = "开始下载";
      setDownloadTarget(pendingUrl, pendingLabel);
    }
  } catch (error) {
    console.warn("CDN latest failed, falling back to GitHub Releases", error);
    document.querySelectorAll(".dl-card").forEach((card) => {
      setCardMeta(card, "CDN 暂不可用 · GitHub 回退");
      card.querySelector("[data-open-download]")?.addEventListener("click", () => {
        pendingUrl = GITHUB_RELEASES_URL;
        pendingLabel = "前往发布页";
        setDownloadTarget(pendingUrl, pendingLabel);
        openModal();
      });
    });
    setDownloadTarget(GITHUB_RELEASES_URL, "前往发布页");
  }

  document.querySelectorAll("[data-close-modal]").forEach((button) => {
    button.addEventListener("click", closeModal);
  });

  const go = document.getElementById("dl-go");
  if (go) {
    go.addEventListener("click", () => closeModal());
  }

  document.getElementById("dl-confirm")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeModal();
  });
});
