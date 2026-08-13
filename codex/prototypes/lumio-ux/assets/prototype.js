/* ============================================================
   Lumio Codex UX 原型 · 共享脚本
   纯演示逻辑：无网络请求、无持久化（仅内存假数据）。
   ============================================================ */

(function () {
  "use strict";

  const PAGES = [
    { group: "入口", items: [{ path: "index.html", label: "原型总入口" }] },
    { group: "官网", items: [{ path: "site/index.html", label: "官网首页" }] },
    {
      group: "App · 账户",
      items: [
        { path: "app/signed-out.html", label: "未登录首页" },
        { path: "app/register.html", label: "注册" },
        { path: "app/login.html", label: "登录（含 2FA）" },
        { path: "app/provisioning.html", label: "自动配置" },
      ],
    },
    {
      group: "App · 主体",
      items: [
        { path: "app/home-online.html", label: "首页 · 在线" },
        { path: "app/home-offline.html", label: "首页 · 离线" },
        { path: "app/payment-handoff.html", label: "充值交接" },
        { path: "app/repair.html", label: "需要检查配置" },
        { path: "app/settings.html", label: "设置" },
      ],
    },
  ];

  // 克制原则：logo 单色中性，不使用品牌渐变
  const LOGO_SVG =
    '<svg viewBox="0 0 24 24" fill="none" aria-hidden="true">' +
    '<path d="M7 3.5v13a4 4 0 0 0 4 4h6" stroke="#d1d5e0" stroke-width="2.4" stroke-linecap="round"/>' +
    '<circle cx="17" cy="7" r="2.1" fill="#d1d5e0" opacity="0.85"/>' +
    "</svg>";

  const root = document.body.dataset.root || "";
  const currentPage = document.body.dataset.page || "";

  /* ---------- 品牌 logo 注入 ---------- */
  document.querySelectorAll(".lx-logo").forEach((el) => {
    if (!el.innerHTML.trim()) el.innerHTML = LOGO_SVG;
  });

  /* ---------- Toast ---------- */
  let toastBox = null;
  function toast(message, type, code) {
    if (!toastBox) {
      toastBox = document.createElement("div");
      toastBox.className = "lx-toasts";
      document.body.appendChild(toastBox);
    }
    while (toastBox.children.length >= 3) toastBox.firstChild.remove();
    const el = document.createElement("div");
    el.className = "lx-toast" + (type ? " is-" + type : "");
    el.innerHTML =
      "<span>" + message + "</span>" + (code ? '<span class="code">' + code + "</span>" : "");
    toastBox.appendChild(el);
    setTimeout(() => el.remove(), 4000);
  }

  /* ---------- 模态层 ---------- */
  function openModal(id) {
    const m = document.getElementById(id);
    if (m) m.classList.add("is-open");
  }
  function closeModal(id) {
    const m = document.getElementById(id);
    if (m) m.classList.remove("is-open");
  }
  document.addEventListener("click", (e) => {
    const backdrop = e.target.closest(".lx-modal-backdrop");
    if (backdrop && e.target === backdrop) backdrop.classList.remove("is-open");
    const closer = e.target.closest("[data-close-modal]");
    if (closer) closeModal(closer.dataset.closeModal);
    const opener = e.target.closest("[data-open-modal]");
    if (opener) openModal(opener.dataset.openModal);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      document
        .querySelectorAll(".lx-modal-backdrop.is-open")
        .forEach((m) => m.classList.remove("is-open"));
    }
  });

  /* ---------- 验证码倒计时 ---------- */
  function countdown(button, seconds, idleLabel) {
    let left = seconds;
    button.disabled = true;
    const original = idleLabel || button.textContent;
    const tick = () => {
      if (left <= 0) {
        button.disabled = false;
        button.textContent = "重新发送";
        return;
      }
      button.textContent = "重新发送 (" + left + "s)";
      left -= 1;
      setTimeout(tick, 1000);
    };
    button.dataset.idleLabel = original;
    tick();
  }

  /* ---------- 2FA 分格输入 ---------- */
  function bindOtp(container, onComplete) {
    const inputs = Array.from(container.querySelectorAll("input"));
    const value = () => inputs.map((i) => i.value).join("");
    inputs.forEach((input, idx) => {
      input.addEventListener("input", () => {
        input.value = input.value.replace(/\D/g, "").slice(-1);
        if (input.value && idx < inputs.length - 1) inputs[idx + 1].focus();
        if (value().length === inputs.length) onComplete(value());
      });
      input.addEventListener("keydown", (e) => {
        if (e.key === "Backspace" && !input.value && idx > 0) inputs[idx - 1].focus();
      });
      input.addEventListener("paste", (e) => {
        e.preventDefault();
        const digits = (e.clipboardData.getData("text") || "").replace(/\D/g, "").slice(0, inputs.length);
        digits.split("").forEach((d, i) => (inputs[i].value = d));
        const next = inputs[Math.min(digits.length, inputs.length - 1)];
        next.focus();
        if (digits.length === inputs.length) onComplete(value());
      });
    });
    return {
      clear() {
        inputs.forEach((i) => (i.value = ""));
        inputs[0].focus();
      },
    };
  }

  /* ---------- 原型导航浮层 ---------- */
  function buildProtoNav() {
    const nav = document.createElement("aside");
    nav.className = "proto-nav";
    nav.setAttribute("aria-label", "原型导航");

    const head = document.createElement("button");
    head.type = "button";
    head.className = "proto-nav-head";
    head.innerHTML = "<span>◈ 原型导航</span><span class=\"caret\">▾</span>";
    head.addEventListener("click", () => nav.classList.toggle("is-collapsed"));

    const body = document.createElement("div");
    body.className = "proto-nav-body";

    PAGES.forEach((group) => {
      const g = document.createElement("p");
      g.className = "proto-nav-group";
      g.textContent = group.group;
      body.appendChild(g);
      group.items.forEach((item) => {
        const a = document.createElement("a");
        a.href = root + item.path;
        a.textContent = item.label;
        if (item.path === currentPage) a.classList.add("is-current");
        body.appendChild(a);
      });
    });

    const variants = window.protoVariants || [];
    if (variants.length) {
      const g = document.createElement("p");
      g.className = "proto-nav-group";
      g.textContent = "本页状态变体";
      body.appendChild(g);
      variants.forEach((v) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "variant";
        b.textContent = v.label;
        b.dataset.hash = v.hash;
        b.addEventListener("click", () => {
          if (location.hash === v.hash) {
            history.replaceState(null, "", location.pathname);
          } else {
            location.hash = v.hash;
          }
          window.dispatchEvent(new HashChangeEvent("hashchange"));
        });
        body.appendChild(b);
      });
      const sync = () => {
        body.querySelectorAll("button.variant").forEach((b) => {
          b.classList.toggle("is-active", location.hash === b.dataset.hash);
        });
      };
      window.addEventListener("hashchange", sync);
      sync();
    }

    nav.appendChild(head);
    nav.appendChild(body);
    document.body.appendChild(nav);
  }

  if (document.body.dataset.protoNav !== "off") buildProtoNav();

  /* ---------- 导出 ---------- */
  window.Proto = { toast, openModal, closeModal, countdown, bindOtp };
})();
