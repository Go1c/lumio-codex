(() => {
  const root = document.documentElement;
  const params = new URLSearchParams(location.search);
  const stored = localStorage.getItem("bc-theme");
  const theme = params.get("theme") || stored || "light";
  root.setAttribute("data-theme", theme);

  document.querySelectorAll("[data-theme-set]").forEach((button) => {
    const value = button.getAttribute("data-theme-set");
    if (value === theme) button.classList.add("is-on");
    button.addEventListener("click", () => {
      root.setAttribute("data-theme", value);
      localStorage.setItem("bc-theme", value);
      document.querySelectorAll("[data-theme-set]").forEach((el) => {
        el.classList.toggle("is-on", el.getAttribute("data-theme-set") === value);
      });
    });
  });

  document.querySelectorAll("[data-toggle]").forEach((button) => {
    button.addEventListener("click", () => {
      if (button.disabled) return;
      button.classList.toggle("is-on");
    });
  });

  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  function showViews(scope, name) {
    scope.querySelectorAll("[data-view]").forEach((el) => {
      const on = el.getAttribute("data-view") === name;
      el.hidden = !on;
    });
  }

  function later(ms) {
    return new Promise((resolve) => {
      window.setTimeout(resolve, reduceMotion ? 0 : ms);
    });
  }

  const installRoot = document.querySelector("[data-install]");
  if (installRoot) {
    const views = ["idle", "place", "download", "verify", "install", "fail"];
    const setInstall = (name) => {
      if (!views.includes(name)) return;
      showViews(installRoot, name);
      installRoot.setAttribute("data-now", name);
    };

    const start = params.get("state") || "idle";
    setInstall(views.includes(start) ? start : "idle");

    installRoot.querySelectorAll(".place").forEach((el) => {
      el.addEventListener("click", () => {
        installRoot.querySelectorAll(".place").forEach((item) => item.classList.remove("is-on"));
        el.classList.add("is-on");
      });
    });

    installRoot.querySelectorAll("[data-install-go]").forEach((el) => {
      el.addEventListener("click", async (event) => {
        event.preventDefault();
        const next = el.getAttribute("data-install-go");
        if (next === "run") {
          setInstall("download");
          await later(1100);
          if (installRoot.getAttribute("data-now") !== "download") return;
          setInstall("verify");
          await later(800);
          if (installRoot.getAttribute("data-now") !== "verify") return;
          setInstall("install");
          return;
        }
        setInstall(next);
      });
    });
  }

  const wizard = document.querySelector("[data-wizard]");
  if (wizard) {
    const steps = ["host", "probe", "setup", "sync"];
    const legend = wizard.querySelectorAll("[data-wiz-step]");

    const paintLegend = (name) => {
      const index = steps.indexOf(name);
      legend.forEach((item) => {
        const id = item.getAttribute("data-wiz-step");
        const at = steps.indexOf(id);
        item.classList.toggle("is-on", id === name);
        item.classList.toggle("is-done", at >= 0 && at < index);
      });
      wizard.querySelectorAll(".sheet-steps i").forEach((bar, i) => {
        bar.classList.toggle("is-on", steps[i] === name);
        bar.classList.toggle("is-done", i < index);
      });
    };

    const setStep = (name) => {
      const id = steps.includes(name) ? name : "host";
      showViews(wizard, id);
      paintLegend(id);
      wizard.setAttribute("data-now", id);
    };

    const requested = params.get("step") || "host";
    setStep(requested);
    if (params.get("fail") === "1" && requested === "probe") {
      const ok = wizard.querySelector("[data-view='probe'] [data-probe='ok']");
      const bad = wizard.querySelector("[data-view='probe'] [data-probe='fail']");
      if (ok) ok.hidden = true;
      if (bad) bad.hidden = false;
    }

    wizard.querySelectorAll("[data-wiz-go]").forEach((el) => {
      el.addEventListener("click", async (event) => {
        if (el.tagName === "A" || el.type === "submit") event.preventDefault();
        const next = el.getAttribute("data-wiz-go");
        if (next === "probe") {
          setStep("probe");
          const ok = wizard.querySelector("[data-probe='ok']");
          const bad = wizard.querySelector("[data-probe='fail']");
          if (ok) ok.hidden = false;
          if (bad) bad.hidden = true;
          return;
        }
        setStep(next);
      });
    });
  }

  const site = document.querySelector("[data-site]");
  if (site) {
    const tabs = ["codex", "claude"];
    const fromHash = location.hash.replace("#", "");
    const start = tabs.includes(params.get("tab") || "")
      ? params.get("tab")
      : tabs.includes(fromHash)
        ? fromHash
        : "codex";

    const setTab = (name, push) => {
      const id = tabs.includes(name) ? name : "codex";
      site.setAttribute("data-now", id);
      site.querySelectorAll("[data-site-tab]").forEach((el) => {
        const on = el.getAttribute("data-site-tab") === id;
        el.classList.toggle("is-on", on);
        if (el.hasAttribute("aria-selected")) el.setAttribute("aria-selected", on ? "true" : "false");
      });
      site.querySelectorAll("[data-pane]").forEach((el) => {
        el.hidden = el.getAttribute("data-pane") !== id;
      });
      site.querySelectorAll("[data-download]").forEach((el) => {
        el.setAttribute("href", id === "claude" ? "#claude-downloads" : "#downloads");
      });
      if (push) {
        const next = new URL(location.href);
        next.searchParams.delete("tab");
        next.hash = id;
        history.replaceState(null, "", `${next.pathname}${next.search}#${id}`);
      }
    };

    setTab(start, false);
    site.querySelectorAll("[data-site-tab]").forEach((el) => {
      el.addEventListener("click", (event) => {
        event.preventDefault();
        setTab(el.getAttribute("data-site-tab"), true);
        const scroller = site.querySelector(".win-main") || window;
        if (scroller === window) window.scrollTo({ top: 0 });
        else scroller.scrollTo({ top: 0 });
      });
    });
    window.addEventListener("hashchange", () => {
      const id = location.hash.replace("#", "");
      if (tabs.includes(id)) setTab(id, false);
    });
  }
})();
