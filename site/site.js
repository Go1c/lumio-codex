const RELEASES_URL = "https://github.com/Go1c/lumio-codex/releases";

function detectPlatform() {
  const ua = navigator.userAgent;
  if (/Macintosh|Mac OS X/i.test(ua)) {
    // Apple Silicon is the common case for new Macs; Intel still gets a card.
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

document.addEventListener("DOMContentLoaded", () => {
  const platform = detectPlatform();
  document.querySelectorAll(".dl-card").forEach((card) => {
    const recommended = card.dataset.platform === platform;
    card.classList.toggle("is-recommended", recommended);
    const chip = card.querySelector("[data-rec-chip]");
    if (chip) chip.hidden = !recommended;
  });

  document.querySelectorAll("[data-open-download]").forEach((button) => {
    button.addEventListener("click", openModal);
  });
  document.querySelectorAll("[data-close-modal]").forEach((button) => {
    button.addEventListener("click", closeModal);
  });

  const go = document.getElementById("dl-go");
  if (go) {
    go.setAttribute("href", RELEASES_URL);
    go.addEventListener("click", () => closeModal());
  }

  document.getElementById("dl-confirm")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeModal();
  });
});
