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

  function viewTokens(el) {
    return (el.getAttribute("data-view") || "").split(/\s+/).filter(Boolean);
  }

  // A view may serve several states, e.g. data-view="ready update".
  function showViews(scope, name) {
    scope.querySelectorAll("[data-view]").forEach((el) => {
      el.hidden = !viewTokens(el).includes(name);
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

  // ——— 右栏文件浏览器：一份实现，三个页面共用 ———

  const FX_ICONS = {
    chev: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3.5 10.5 8 6 12.5"/></svg>',
    dir: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v6.1c0 .7-.5 1.2-1.2 1.2H2.8c-.7 0-1.2-.5-1.2-1.2V4.3Z"/></svg>',
    dirOpen:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v.9"/><path d="M1.6 6.6h12.8l-1.1 5.4c-.1.6-.6 1-1.2 1H3.9c-.6 0-1.1-.4-1.2-1L1.6 6.6Z"/></svg>',
    // 文件图标按类型分：空白页 / 带正文的页 / 代码页 / 配置页。
    file: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z"/><path d="M9.2 2v3.4h3.4"/></svg>',
    fileText:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z"/><path d="M9.2 2v3.4h3.4"/><path d="M5.4 8.2h5.2M5.4 10.4h5.2M5.4 12h3.2" stroke-linecap="round"/></svg>',
    fileCode:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z"/><path d="M9.2 2v3.4h3.4"/><path d="M6.6 8.9 5.2 10.3l1.4 1.4M9.4 8.9l1.4 1.4-1.4 1.4" stroke-linecap="round"/></svg>',
    fileConf:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z"/><path d="M9.2 2v3.4h3.4"/><path d="M7 8.9c-.9 0-.9 1.4-1.7 1.4.8 0 .8 1.4 1.7 1.4M9 8.9c.9 0 .9 1.4 1.7 1.4-.8 0-.8 1.4-1.7 1.4" stroke-linecap="round"/></svg>',
    filter:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><path d="M2.5 4h11M4.5 8h7M6.5 12h3"/></svg>',
    search:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="7" cy="7" r="4.4"/><path d="m10.4 10.4 3.1 3.1"/></svg>',
    collapse:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 3.5h8M6.5 8h8M6.5 12.5h8"/><path d="M1.5 5 3 6.5 4.5 5"/><path d="M1.5 11 3 9.5 4.5 11"/></svg>',
    refresh:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M13.6 8a5.6 5.6 0 1 1-1.7-4"/><path d="M13.9 2.2v3.2h-3.2"/></svg>',
    more: '<svg viewBox="0 0 16 16" fill="currentColor"><circle cx="3.4" cy="8" r="1.2"/><circle cx="8" cy="8" r="1.2"/><circle cx="12.6" cy="8" r="1.2"/></svg>',
    filePlus:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z"/><path d="M9.2 2v3.4h3.4"/><path d="M8 8.4v3.2M6.4 10h3.2" stroke-linecap="round"/></svg>',
    folderPlus:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M1.6 4.3c0-.7.5-1.2 1.2-1.2h2.5c.4 0 .7.1.9.4l.8 1h5.2c.7 0 1.2.5 1.2 1.2v6.1c0 .7-.5 1.2-1.2 1.2H2.8c-.7 0-1.2-.5-1.2-1.2V4.3Z"/><path d="M8 7.6v3.4M6.3 9.3h3.4" stroke-linecap="round"/></svg>',
    copy: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><rect x="5.6" y="5.6" width="8" height="8" rx="1.4"/><path d="M11 5.6V3.8c0-.8-.6-1.4-1.4-1.4H3.8c-.8 0-1.4.6-1.4 1.4v5.8c0 .8.6 1.4 1.4 1.4h1.8"/></svg>',
    clipboard:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><rect x="4.4" y="2.6" width="7.2" height="11" rx="1.4"/><path d="M6.4 2.6V2c0-.4.3-.7.7-.7h1.8c.4 0 .7.3.7.7v.6"/><path d="M6.6 7.2h2.8M6.6 9.8h2.8" stroke-linecap="round"/></svg>',
    duplicate:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M6.4 4.6h5.4c.8 0 1.4.6 1.4 1.4v7c0 .8-.6 1.4-1.4 1.4H6.4c-.8 0-1.4-.6-1.4-1.4V6c0-.8.6-1.4 1.4-1.4Z"/><path d="M3.2 11V3.6c0-.8.6-1.4 1.4-1.4h5.2"/></svg>',
    fileEye:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M9.3 1.9H4.5c-.7 0-1.2.5-1.2 1.2v9.8c0 .7.5 1.2 1.2 1.2h7c.7 0 1.2-.5 1.2-1.2V5.3L9.3 1.9Z"/><path d="M9.2 2v3.4h3.4"/></svg>',
    globe:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2"><circle cx="8" cy="8" r="6.1"/><path d="M1.9 8h12.2"/><path d="M8 1.9c1.7 1.7 2.6 3.8 2.6 6.1S9.7 12.4 8 14.1C6.3 12.4 5.4 10.3 5.4 8S6.3 3.6 8 1.9Z"/></svg>',
    eye: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M1.4 8S3.8 3.6 8 3.6 14.6 8 14.6 8 12.2 12.4 8 12.4 1.4 8 1.4 8Z"/><circle cx="8" cy="8" r="1.9"/></svg>',
    external:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.4 2.4h4.2v4.2"/><path d="m13.6 2.4-6 6"/><path d="M12 9.6v3.2c0 .5-.4.9-.9.9H3.3c-.5 0-.9-.4-.9-.9V5c0-.5.4-.9.9-.9h3.2"/></svg>',
    pencil:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M11.2 2.6a1.6 1.6 0 0 1 2.2 2.2l-7.5 7.5-3 .8.8-3 7.5-7.5Z"/></svg>',
    trash:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.6 4.4h10.8"/><path d="M6.2 4.4V2.9c0-.4.3-.7.7-.7h2.2c.4 0 .7.3.7.7v1.5"/><path d="M4.2 4.4l.6 8.4c0 .5.5.9 1 .9h4.4c.5 0 1-.4 1-.9l.6-8.4"/></svg>',
  };

  // 一棵示意项目树。tag 是本机与服务器的差别，text 用于「内容」检索。
  const FX_TREE = [
    { path: ".claude", kind: "dir" },
    { path: ".claude/settings.json", kind: "file", text: '{ "permissions": { "allow": [] } }' },
    { path: ".github", kind: "dir" },
    { path: ".github/workflows", kind: "dir" },
    { path: ".github/workflows/ci.yml", kind: "file", text: "runs-on: ubuntu-latest" },
    { path: "src", kind: "dir", open: true },
    { path: "src/app.ts", kind: "file", text: 'import { withRetry } from "./lib/retry";' },
    { path: "src/lib", kind: "dir", open: true },
    { path: "src/lib/sync.ts", kind: "file", tag: "已改", text: "export async function pullRemote() {" },
    { path: "src/lib/retry.ts", kind: "file", tag: "新增", text: "export async function withRetry<T>(" },
    { path: "src/lib/paths.ts", kind: "file", text: "export function localRoot(name: string) {" },
    { path: "tests", kind: "dir" },
    { path: "tests/sync.test.ts", kind: "file", text: 'test("pullRemote 会重试三次"' },
    { path: "docs", kind: "dir" },
    { path: "docs/setup.md", kind: "file", text: "# 部署说明" },
    { path: "README.md", kind: "file", text: "# my-project" },
    { path: "package.json", kind: "file", text: '"name": "my-project"' },
    { path: "tsconfig.json", kind: "file", text: '"strict": true' },
    { path: ".gitignore", kind: "file", text: "node_modules" },
    { path: ".DS_Store", kind: "file", text: "" },
  ];

  function fxName(path) {
    return path.slice(path.lastIndexOf("/") + 1);
  }

  function fxDepth(path) {
    return path.split("/").length - 1;
  }

  function fxParent(path) {
    const at = path.lastIndexOf("/");
    return at < 0 ? "" : path.slice(0, at);
  }

  // 目录在前，同级按名字排（不分大小写），和文件管理器一致。
  const FX_SORTED = [...FX_TREE].sort((a, b) => {
    const ap = a.path.split("/");
    const bp = b.path.split("/");
    for (let i = 0; i < Math.max(ap.length, bp.length); i += 1) {
      if (ap[i] === bp[i]) continue;
      if (ap[i] === undefined) return -1;
      if (bp[i] === undefined) return 1;
      const aLeaf = i === ap.length - 1 && a.kind === "file";
      const bLeaf = i === bp.length - 1 && b.kind === "file";
      if (aLeaf !== bLeaf) return aLeaf ? 1 : -1;
      return ap[i].localeCompare(bp[i], "zh-Hans-CN", { sensitivity: "base" });
    }
    return 0;
  });

  const FX_TEXT = new Set(["md", "txt", "yml", "yaml", "gitignore", "env", "log"]);
  const FX_CODE = new Set(["ts", "tsx", "js", "jsx", "mjs", "rs", "py", "sh"]);
  const FX_CONF = new Set(["json", "toml", "lock"]);

  function fxGlyph(node, isOpen) {
    if (node.kind === "dir") return isOpen ? FX_ICONS.dirOpen : FX_ICONS.dir;
    const name = fxName(node.path);
    const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
    if (FX_CODE.has(ext)) return FX_ICONS.fileCode;
    if (FX_CONF.has(ext)) return FX_ICONS.fileConf;
    if (FX_TEXT.has(ext)) return FX_ICONS.fileText;
    return FX_ICONS.file;
  }

  function escapeHtml(value) {
    return value
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function escapeRe(value) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }

  // 查找条上的三个开关：Aa 区分大小写、ab 全字匹配、.* 正则。
  function fxMatcher(query, flags) {
    if (!query) return null;
    let source = flags.regex ? query : escapeRe(query);
    if (flags.word) source = `\\b(?:${source})\\b`;
    try {
      return new RegExp(source, flags.case ? "g" : "gi");
    } catch {
      return "bad";
    }
  }

  function fxHits(text, re) {
    if (!text) return false;
    re.lastIndex = 0;
    return re.test(text);
  }

  function fxMark(text, re) {
    if (!re) return escapeHtml(text);
    let out = "";
    let last = 0;
    re.lastIndex = 0;
    let found;
    while ((found = re.exec(text))) {
      if (found[0] === "") {
        re.lastIndex += 1;
        continue;
      }
      out += `${escapeHtml(text.slice(last, found.index))}<b>${escapeHtml(found[0])}</b>`;
      last = found.index + found[0].length;
    }
    return out + escapeHtml(text.slice(last));
  }

  // 要包含 / 要排除的文件：逗号分隔的 glob，没写斜杠的当成任意层级。
  function fxGlobs(value) {
    return value
      .split(",")
      .map((one) => one.trim())
      .filter(Boolean)
      .map((one) => {
        const pattern = one.includes("/") ? one : `**/${one}`;
        const body = escapeRe(pattern)
          .replace(/\\\*\\\*\//g, "\u0000")
          .replace(/\\\*\\\*/g, "\u0001")
          .replace(/\\\*/g, "[^/]*")
          .replace(/\\\?/g, "[^/]")
          .replace(/\u0000/g, "(?:.*/)?")
          .replace(/\u0001/g, ".*");
        try {
          return new RegExp(`^${body}$`);
        } catch {
          return null;
        }
      })
      .filter(Boolean);
  }

  function fxTagClass(tag) {
    if (tag === "冲突") return "is-conflict";
    if (tag === "新增") return "is-added";
    return "is-changed";
  }

  const FX_CTX_ITEMS = [
    { label: "新文件", icon: "filePlus" },
    { label: "新建文件夹", icon: "folderPlus" },
    { sep: true },
    { label: "复制", icon: "copy" },
    { label: "复制路径", icon: "clipboard", keys: "⌘⌥C" },
    { label: "复制相对路径", icon: "clipboard", keys: "⌘⌥⇧C" },
    { label: "复制", icon: "duplicate" },
    { label: "查看文件", icon: "fileEye" },
    { label: "在内置浏览器中打开", icon: "globe" },
    { label: "打开 Markdown 预览", icon: "eye", only: "md" },
    { label: "在 Finder 中显示", icon: "external" },
    { sep: true },
    { label: "重命名", icon: "pencil", keys: "↵" },
    { label: "删除", icon: "trash", keys: "⌘Backspace, Delete", danger: true },
  ];

  function mountExplorer(fx, readState) {
    const open = new Set(FX_TREE.filter((node) => node.open).map((node) => node.path));
    let query = "";
    let mode = "name";
    let picked = null;
    let onlyChanged = false;
    const flags = { case: false, word: false, regex: false };

    const conflictPaths = (fx.getAttribute("data-fx-conflict") || "").split(/\s+/).filter(Boolean);
    const conflictStates = (fx.getAttribute("data-fx-conflict-states") || "")
      .split(/\s+/)
      .filter(Boolean);
    const blankStates = (fx.getAttribute("data-fx-blank-states") || "").split(/\s+/).filter(Boolean);
    const partialStates = (fx.getAttribute("data-fx-partial-states") || "").split(/\s+/).filter(Boolean);
    const partialNote = fx.getAttribute("data-fx-partial-note") || "正在拉取…";

    fx.innerHTML = `
      <div class="d-fx-head">
        <h3>${escapeHtml(fx.getAttribute("data-fx-root") || "项目")}</h3>
        <button class="d-fx-icon" data-fx-collapse data-tip="全部折叠" type="button" aria-label="全部折叠">${FX_ICONS.collapse}</button>
        <button class="d-fx-icon" data-fx-refresh data-tip="刷新文件管理器" type="button" aria-label="刷新文件管理器">${FX_ICONS.refresh}</button>
        <button class="d-fx-icon" data-fx-more data-tip="更多" type="button" aria-label="更多" aria-expanded="false">${FX_ICONS.more}</button>
        <div class="d-fx-menu" data-fx-menu hidden role="menu">
          <button data-fx-only type="button" role="menuitemcheckbox" aria-checked="false"><span class="tick"></span>只看有改动的</button>
          <button data-fx-expand type="button" role="menuitem"><span class="tick"></span>全部展开</button>
          <hr />
          <button type="button" role="menuitem"><span class="tick"></span>在本机打开项目文件夹</button>
        </div>
      </div>
      <div class="d-fx-search">
        <svg data-fx-find-icon viewBox="0 0 16 16"></svg>
        <input data-fx-query type="search" />
        <span class="d-fx-flags" data-fx-flags hidden>
          <button class="d-fx-flag" data-fx-flag="case" data-tip="区分大小写" type="button" aria-pressed="false">Aa</button>
          <button class="d-fx-flag" data-fx-flag="word" data-tip="全字匹配" type="button" aria-pressed="false"><u>ab</u></button>
          <button class="d-fx-flag" data-fx-flag="regex" data-tip="使用正则表达式" type="button" aria-pressed="false">.*</button>
        </span>
      </div>
      <div class="d-fx-modes" role="tablist" aria-label="检索范围">
        <button class="is-on" data-fx-mode="name" role="tab" type="button" aria-selected="true">名称</button>
        <button data-fx-mode="content" role="tab" type="button" aria-selected="false">内容</button>
      </div>
      <div class="d-fx-globs" data-fx-globs hidden>
        <label>要包含的文件<input data-fx-include type="text" placeholder="要包含的文件（例如 *.ts、src/**）" /></label>
        <label>要排除的文件<input data-fx-exclude type="text" placeholder="要排除的文件（例如 *.min.js、dist）" /></label>
      </div>
      <div class="d-fx-tree" data-fx-tree role="tree"></div>
      <p class="d-fx-foot" data-fx-foot></p>
      <div class="d-fx-ctx" data-fx-ctx hidden role="menu"></div>
    `;

    const treeEl = fx.querySelector("[data-fx-tree]");
    const footEl = fx.querySelector("[data-fx-foot]");
    const queryEl = fx.querySelector("[data-fx-query]");
    const flagsEl = fx.querySelector("[data-fx-flags]");
    const globsEl = fx.querySelector("[data-fx-globs]");
    const includeEl = fx.querySelector("[data-fx-include]");
    const excludeEl = fx.querySelector("[data-fx-exclude]");
    const ctxEl = fx.querySelector("[data-fx-ctx]");

    const tagFor = (node, state) => {
      if (conflictPaths.includes(node.path) && conflictStates.includes(state)) return "冲突";
      return node.tag || null;
    };

    // 名称态是过滤文件名，内容态是全文检索，两套查找条长得不一样。
    const paintFindBar = () => {
      const isName = mode === "name";
      // 每次重新查，上一轮的图标节点已经被换掉了。
      fx.querySelector("[data-fx-find-icon]").outerHTML = (
        isName ? FX_ICONS.filter : FX_ICONS.search
      ).replace("<svg", "<svg data-fx-find-icon");
      const input = fx.querySelector("[data-fx-query]");
      input.placeholder = isName ? "查找文件" : "搜索";
      input.setAttribute("aria-label", isName ? "查找文件" : "在文件中搜索");
      flagsEl.hidden = isName;
      globsEl.hidden = isName;
    };

    const closeCtx = () => {
      ctxEl.hidden = true;
    };

    const openCtx = (node, x, y) => {
      const ext = fxName(node.path).split(".").pop().toLowerCase();
      ctxEl.innerHTML = FX_CTX_ITEMS.filter((item) => !item.only || item.only === ext)
        .map((item) =>
          item.sep
            ? "<hr />"
            : `<button type="button" role="menuitem"${item.danger ? ' class="is-danger"' : ""}><span class="ico">${
                FX_ICONS[item.icon]
              }</span><span class="lbl">${escapeHtml(item.label)}</span>${
                item.keys ? `<span class="keys">${escapeHtml(item.keys)}</span>` : ""
              }</button>`,
        )
        .join("");
      ctxEl.hidden = false;
      const box = fx.getBoundingClientRect();
      const w = ctxEl.offsetWidth || 240;
      const h = ctxEl.offsetHeight || 380;
      let left = x - box.left;
      let top = y - box.top;
      if (left + w > box.width) left = Math.max(4, box.width - w - 4);
      if (top + h > box.height) top = Math.max(4, box.height - h - 4);
      ctxEl.style.left = `${left}px`;
      ctxEl.style.top = `${top}px`;
      ctxEl.querySelectorAll("button").forEach((item) => item.addEventListener("click", closeCtx));
    };

    const render = () => {
      const state = readState();
      closeCtx();

      if (blankStates.includes(state)) {
        treeEl.innerHTML = '<p class="d-fx-empty">还没选项目。左边点一下就把文件列在这。</p>';
        footEl.textContent = "";
        return;
      }

      const re = fxMatcher(query, flags);
      if (re === "bad") {
        treeEl.innerHTML = '<p class="d-fx-empty">正则写法有问题，改一下再看。</p>';
        footEl.textContent = "";
        return;
      }

      // 内容态还没输关键词时，不列文件，只说要干什么。
      if (mode === "content" && !query) {
        treeEl.innerHTML = '<p class="d-fx-hint">输入要在文件中搜索的内容</p>';
        footEl.textContent = "";
        return;
      }

      const hidden = (path) => {
        const parts = path.split("/");
        for (let i = 1; i < parts.length; i += 1) {
          if (!open.has(parts.slice(0, i).join("/"))) return true;
        }
        return false;
      };

      let rows = [];
      if (mode === "content") {
        const inc = fxGlobs(includeEl.value);
        const exc = fxGlobs(excludeEl.value);
        rows = FX_SORTED.filter(
          (node) =>
            node.kind === "file" &&
            fxHits(node.text, re) &&
            (inc.length === 0 || inc.some((g) => g.test(node.path))) &&
            !exc.some((g) => g.test(node.path)),
        );
      } else if (query) {
        rows = FX_SORTED.filter((node) => fxHits(fxName(node.path), re));
      } else if (onlyChanged) {
        rows = FX_SORTED.filter((node) => node.kind === "file" && tagFor(node, state));
      } else {
        rows = FX_SORTED.filter((node) => !hidden(node.path));
      }

      const partial = !query && !onlyChanged && partialStates.includes(state);
      if (partial) rows = rows.slice(0, 6);

      if (rows.length === 0) {
        treeEl.innerHTML = query
          ? `<p class="d-fx-empty">没有匹配「${escapeHtml(query)}」的${
              mode === "name" ? "文件名" : "文件内容"
            }。</p>`
          : '<p class="d-fx-empty">两边一模一样，没有改动。</p>';
        footEl.textContent = "";
        return;
      }

      // 搜索结果和「只看有改动的」是平的，显示完整路径；否则按层级缩进只显示名字。
      const flat = Boolean(query) || onlyChanged;
      treeEl.innerHTML = rows
        .map((node) => {
          const depth = flat ? 0 : fxDepth(node.path);
          const isDir = node.kind === "dir";
          const isOpen = isDir && open.has(node.path);
          const label = flat ? node.path : fxName(node.path);
          const tag = tagFor(node, state);
          const chev = `<span class="d-fx-chev">${isDir ? FX_ICONS.chev : ""}</span>`;
          const name = mode === "name" && query ? fxMark(label, re) : escapeHtml(label);
          const hit =
            mode === "content"
              ? `<span class="d-fx-hit" style="--depth:${depth}">${fxMark(node.text || "", re)}</span>`
              : "";
          return `<button class="d-fx-row${isDir ? " is-dir" : ""}${isOpen ? " is-open" : ""}${
            node.path === picked ? " is-on" : ""
          }" style="--depth:${depth}" data-fx-path="${escapeHtml(node.path)}" data-fx-kind="${
            node.kind
          }" role="treeitem"${isDir ? ` aria-expanded="${isOpen}"` : ""} type="button">${chev}<span class="d-fx-glyph">${fxGlyph(
            node,
            isOpen,
          )}</span><span class="d-fx-name">${name}</span>${
            tag ? `<span class="d-fx-tag ${fxTagClass(tag)}">${tag}</span>` : ""
          }</button>${hit}`;
        })
        .join("");

      const conflicts = FX_TREE.filter((node) => tagFor(node, state) === "冲突").length;
      footEl.textContent = query
        ? `${rows.length} 个结果 · ${mode === "name" ? "按名称" : "按内容"}`
        : onlyChanged
          ? `只看有改动的 · ${rows.length} 个文件`
          : partial
            ? partialNote
            : conflicts > 0
              ? `本机与服务器合并显示 · ${conflicts} 个文件有冲突，在底栏处理`
              : "本机与服务器合并显示。角标是两边的差别。";

      treeEl.querySelectorAll("[data-fx-path]").forEach((row) => {
        const path = row.getAttribute("data-fx-path");
        const node = FX_TREE.find((one) => one.path === path);
        row.addEventListener("click", () => {
          picked = path;
          if (row.getAttribute("data-fx-kind") === "dir" && !flat) {
            if (open.has(path)) open.delete(path);
            else open.add(path);
          }
          render();
        });
        row.addEventListener("contextmenu", (event) => {
          event.preventDefault();
          picked = path;
          treeEl.querySelectorAll(".d-fx-row").forEach((el) => {
            el.classList.toggle("is-on", el === row);
          });
          openCtx(node, event.clientX, event.clientY);
        });
      });
    };

    queryEl.addEventListener("input", () => {
      query = fx.querySelector("[data-fx-query]").value.trim();
      render();
    });
    [includeEl, excludeEl].forEach((el) => el.addEventListener("input", render));

    fx.querySelectorAll("[data-fx-flag]").forEach((button) => {
      button.addEventListener("click", () => {
        const key = button.getAttribute("data-fx-flag");
        flags[key] = !flags[key];
        button.classList.toggle("is-on", flags[key]);
        button.setAttribute("aria-pressed", String(flags[key]));
        render();
      });
    });

    fx.querySelectorAll("[data-fx-mode]").forEach((button) => {
      button.addEventListener("click", () => {
        mode = button.getAttribute("data-fx-mode");
        fx.querySelectorAll("[data-fx-mode]").forEach((el) => {
          const on = el === button;
          el.classList.toggle("is-on", on);
          el.setAttribute("aria-selected", String(on));
        });
        paintFindBar();
        render();
      });
    });

    fx.querySelector("[data-fx-collapse]").addEventListener("click", () => {
      open.clear();
      picked = null;
      render();
    });
    const refreshEl = fx.querySelector("[data-fx-refresh]");
    refreshEl.addEventListener("click", () => {
      refreshEl.classList.remove("is-spin");
      void refreshEl.offsetWidth;
      refreshEl.classList.add("is-spin");
      render();
    });

    const moreEl = fx.querySelector("[data-fx-more]");
    const menuEl = fx.querySelector("[data-fx-menu]");
    const closeMenu = () => {
      menuEl.hidden = true;
      moreEl.setAttribute("aria-expanded", "false");
    };
    moreEl.addEventListener("click", (event) => {
      event.stopPropagation();
      menuEl.hidden = !menuEl.hidden;
      moreEl.setAttribute("aria-expanded", String(!menuEl.hidden));
    });
    const onlyEl = fx.querySelector("[data-fx-only]");
    onlyEl.addEventListener("click", () => {
      onlyChanged = !onlyChanged;
      onlyEl.querySelector(".tick").textContent = onlyChanged ? "✓" : "";
      onlyEl.setAttribute("aria-checked", String(onlyChanged));
      closeMenu();
      render();
    });
    fx.querySelector("[data-fx-expand]").addEventListener("click", () => {
      FX_TREE.forEach((node) => {
        if (node.kind === "dir") open.add(node.path);
      });
      closeMenu();
      render();
    });
    menuEl
      .querySelectorAll("button:not([data-fx-only]):not([data-fx-expand])")
      .forEach((item) => item.addEventListener("click", closeMenu));

    document.addEventListener("click", (event) => {
      if (!menuEl.hidden && !fx.querySelector(".d-fx-head").contains(event.target)) closeMenu();
      if (!ctxEl.hidden && !ctxEl.contains(event.target)) closeCtx();
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        closeMenu();
        closeCtx();
      }
    });
    treeEl.addEventListener("contextmenu", (event) => {
      if (!event.target.closest("[data-fx-path]")) event.preventDefault();
    });

    paintFindBar();
    return render;
  }

  // ——— 左栏：服务器分组 → 项目。会话不在这里，会话在顶部标签条。 ———

  const RAIL_ICONS = {
    chev: FX_ICONS.chev,
    server:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><rect x="2" y="2.6" width="12" height="4.6" rx="1.2"/><rect x="2" y="8.8" width="12" height="4.6" rx="1.2"/><path d="M4.4 4.9h.01M4.4 11.1h.01" stroke-linecap="round" stroke-width="1.6"/></svg>',
    folder: FX_ICONS.dir,
    plus: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><path d="M8 3.6v8.8M3.6 8h8.8"/></svg>',
  };

  // 服务器是连接，项目是这台服务器上的一个文件夹。一台服务器可以有 N 个项目。
  const CC_SERVERS = [
    {
      host: "108.80.81.15",
      alias: "生产机",
      claude: "2.1.228",
      login: true,
      online: true,
      projects: [
        { name: "my-project", dir: "~/bestcodex/my-project", when: "刚刚", live: 2, active: true },
        { name: "docs-site", dir: "~/bestcodex/docs-site", when: "3 分钟前", live: 1 },
        { name: "api-gateway", dir: "~/bestcodex/api-gateway", when: "昨天", live: 0 },
      ],
    },
    {
      host: "10.0.4.22",
      alias: "测试机",
      claude: "2.1.228",
      login: true,
      online: false,
      projects: [
        { name: "cron-jobs", dir: "~/bestcodex/cron-jobs", when: "2 天前", live: 0 },
        { name: "scratch", dir: "~/bestcodex/scratch", when: "上周", live: 0 },
      ],
    },
    {
      host: "192.168.1.40",
      alias: "家里那台",
      claude: null,
      login: false,
      online: false,
      projects: [{ name: "sandbox", dir: "~/bestcodex/sandbox", when: "从没连过", live: 0 }],
    },
  ];

  function mountRail(rail, readState) {
    // 断开的服务器默认收起；当前项目所在的那组永远展开。
    const open = new Set(CC_SERVERS.filter((s) => s.online).map((s) => s.host));
    let picked = "108.80.81.15/my-project";

    const render = () => {
      const state = readState();
      const single = CC_SERVERS.length === 1;

      const projLine = (server, project, key) => {
        let st = `已同步 · ${project.when}`;
        let cls = "";
        if (!server.online) {
          st = project.when === "从没连过" ? "未连接 · 点一下就连" : `未连接 · ${project.when}`;
        } else if (key === picked) {
          if (state === "conflicts") {
            st = "2 个冲突待处理";
            cls = "is-warn";
          } else if (state === "term-fail") {
            st = "终端没能打开";
            cls = "is-bad";
          } else {
            cls = "is-ok";
          }
        }
        return `<span class="st ${cls}">${st}</span>`;
      };

      rail.innerHTML = `
        <div class="d-rail-head">
          <h2>服务器与项目</h2>
        </div>
        <div class="d-rail-body">
          ${CC_SERVERS.map((server) => {
            // 当前项目所在的组不许收起。
            const holdsActive = server.projects.some((p) => `${server.host}/${p.name}` === picked);
            const isOpen = single || holdsActive || open.has(server.host);
            const meta = server.online
              ? `Claude ${server.claude} · ${server.login ? "已登录" : "未登录"}`
              : server.claude
                ? "未连接"
                : "Claude 未装";
            const head = single
              ? ""
              : `<button class="d-srv${isOpen ? " is-open" : ""}${
                  server.online ? " is-on" : ""
                }" data-srv="${server.host}" type="button" aria-expanded="${isOpen}">
                  <span class="chev">${RAIL_ICONS.chev}</span>
                  <i class="d-dot${server.online ? " is-ok" : ""}" aria-hidden="true"></i>
                  <span class="host">${server.host}</span>
                  <span class="meta">${meta}</span>
                </button>`;
            const body = isOpen
              ? `<div class="d-srv-body">
                  ${server.projects
                    .map((project) => {
                      const key = `${server.host}/${project.name}`;
                      return `<button class="d-proj${key === picked ? " is-on" : ""}" data-proj="${key}" type="button">
                        <span class="k"><span class="glyph">${RAIL_ICONS.folder}</span>${project.name}${
                          project.live > 0
                            ? `<i class="d-live" title="有 ${project.live} 个对话在跑" aria-label="有 ${project.live} 个对话在跑"></i>`
                            : ""
                        }</span>
                        <span class="dir">${project.dir}</span>
                        ${projLine(server, project, key)}
                      </button>`;
                    })
                    .join("")}
                  <button class="d-newproj" data-newproj="${server.host}" type="button">
                    <span class="glyph">${RAIL_ICONS.plus}</span>新建项目
                  </button>
                </div>`
              : "";
            return `<section class="d-srv-group">${head}${body}</section>`;
          }).join("")}
        </div>
        <div class="d-rail-foot">
          <a class="btn is-sm" href="cc-connect.html"
            ><span class="glyph">${RAIL_ICONS.plus}</span>连接新服务器</a
          >
        </div>
      `;

      rail.querySelectorAll("[data-srv]").forEach((el) => {
        el.addEventListener("click", () => {
          const host = el.getAttribute("data-srv");
          if (open.has(host)) open.delete(host);
          else open.add(host);
          render();
        });
      });
      rail.querySelectorAll("[data-proj]").forEach((el) => {
        el.addEventListener("click", () => {
          picked = el.getAttribute("data-proj");
          render();
        });
      });
    };

    return render;
  }

  // ——— 中栏顶部：会话标签条。浏览器那套：点一下切换，+ 新建，× 关闭。 ———

  const TAB_ICONS = {
    chat: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M14 7.4c0 2.8-2.7 5-6 5-.7 0-1.4-.1-2-.3L3 13.4l.8-2.3C2.7 10.2 2 8.9 2 7.4c0-2.8 2.7-5 6-5s6 2.2 6 5Z"/></svg>',
    close:
      '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><path d="M4.6 4.6l6.8 6.8M11.4 4.6l-6.8 6.8"/></svg>',
    plus: RAIL_ICONS.plus,
  };

  // 标题先是默认的，等你发出第一句话再换成那句话的前几个字。
  const CC_TABS = [
    { id: 1, title: "帮我把 sync.ts 里的重试逻辑抽成一个函数", named: true, run: false },
    { id: 2, title: null, named: false, run: true },
  ];

  // 中日韩字符按两个宽度算，免得四个标签一排看不出谁是谁。
  function clipWidth(text, budget) {
    let width = 0;
    let out = "";
    for (const ch of text) {
      width += /[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFF60]/.test(ch)
        ? 2
        : 1;
      if (width > budget) return `${out}…`;
      out += ch;
    }
    return out;
  }

  function mountTabs(strip, countEl) {
    const tabs = CC_TABS.map((tab) => ({ ...tab }));
    let live = tabs[0].id;
    let seq = tabs.length;
    let asking = null;

    const render = () => {
      strip.innerHTML = `
        <div class="d-tabs-strip" role="tablist" aria-label="对话">
          ${tabs
            .map(
              (tab) => `<button class="d-tab${tab.id === live ? " is-on" : ""}${
                tab.run ? " is-run" : ""
              }" data-tab="${tab.id}" role="tab" aria-selected="${tab.id === live}" type="button">
                <span class="glyph">${TAB_ICONS.chat}</span>
                <span class="t">${escapeHtml(clipWidth(tab.title || "新对话", 22))}</span>
                <span class="x" data-close="${tab.id}" role="button" aria-label="关闭这个对话">${TAB_ICONS.close}</span>
              </button>`,
            )
            .join("")}
          <button class="d-tab-new" data-newtab type="button" aria-label="新建对话">${TAB_ICONS.plus}</button>
        </div>
        ${
          asking
            ? `<div class="d-tab-ask" role="alertdialog">
                <span>这个对话正在跑，关掉就断了。</span>
                <button class="btn is-sm is-blue" data-ask-yes type="button">还是关掉</button>
                <button class="btn is-sm is-ghost" data-ask-no type="button">先留着</button>
              </div>`
            : ""
        }
      `;

      if (countEl) {
        const running = tabs.filter((t) => t.run).length;
        countEl.textContent = running > 0 ? `对话 ${tabs.length} · ${running} 在跑` : `对话 ${tabs.length}`;
      }

      strip.querySelectorAll("[data-tab]").forEach((el) => {
        el.addEventListener("click", () => {
          live = Number(el.getAttribute("data-tab"));
          asking = null;
          render();
        });
      });
      strip.querySelectorAll("[data-close]").forEach((el) => {
        el.addEventListener("click", (event) => {
          event.stopPropagation();
          const id = Number(el.getAttribute("data-close"));
          const tab = tabs.find((t) => t.id === id);
          // 空闲的直接关，正在跑的先问一句。
          if (tab.run) {
            asking = id;
          } else {
            drop(id);
            return;
          }
          render();
        });
      });
      strip.querySelector("[data-newtab]")?.addEventListener("click", () => {
        seq += 1;
        tabs.push({ id: seq, title: null, named: false, run: false });
        live = seq;
        asking = null;
        render();
      });
      strip.querySelector("[data-ask-yes]")?.addEventListener("click", () => {
        drop(asking);
      });
      strip.querySelector("[data-ask-no]")?.addEventListener("click", () => {
        asking = null;
        render();
      });
    };

    const drop = (id) => {
      const at = tabs.findIndex((t) => t.id === id);
      if (at < 0) return;
      tabs.splice(at, 1);
      asking = null;
      // 关掉最后一个就自动开一个新的，中栏不能空着。
      if (tabs.length === 0) {
        seq += 1;
        tabs.push({ id: seq, title: null, named: false, run: false });
      }
      if (!tabs.some((t) => t.id === live)) {
        live = tabs[Math.min(at, tabs.length - 1)].id;
      }
      render();
    };

    return render;
  }

  const ccLab = document.querySelector("[data-cc]");
  if (ccLab) {
    const states = [];
    ccLab.querySelectorAll("[data-view]").forEach((el) => {
      for (const token of viewTokens(el)) {
        if (!states.includes(token)) states.push(token);
      }
    });

    const now = () => ccLab.getAttribute("data-now") || "";
    const explorer = ccLab.querySelector("[data-fx]");
    const renderExplorer = explorer ? mountExplorer(explorer, now) : null;
    const railEl = ccLab.querySelector("[data-rail]");
    const renderRail = railEl ? mountRail(railEl, now) : null;
    const tabsEl = ccLab.querySelector("[data-tabs]");
    const renderTabs = tabsEl
      ? mountTabs(tabsEl, ccLab.querySelector("[data-tabs-count]"))
      : null;

    const setState = (name) => {
      const id = states.includes(name) ? name : states[0];
      showViews(ccLab, id);
      ccLab.setAttribute("data-now", id);
      ccLab.querySelectorAll("[data-when]").forEach((el) => {
        el.hidden = !el.getAttribute("data-when").split(" ").includes(id);
      });
      ccLab.querySelectorAll("[data-cc-go]").forEach((el) => {
        el.classList.toggle("is-on", el.getAttribute("data-cc-go") === id);
      });
      renderExplorer?.();
      renderRail?.();
      renderTabs?.();
    };

    setState(params.get("state") || states[0]);
    ccLab.querySelectorAll("[data-cc-go]").forEach((el) => {
      el.addEventListener("click", (event) => {
        event.preventDefault();
        setState(el.getAttribute("data-cc-go"));
      });
    });

    // Bottom bar raises a drawer for anything that does not fit in small type.
    const drawer = ccLab.querySelector("[data-drawer]");
    if (drawer) {
      const openDrawer = (name) => {
        drawer.hidden = false;
        drawer.querySelectorAll("[data-drawer-view]").forEach((el) => {
          el.hidden = el.getAttribute("data-drawer-view") !== name;
        });
        drawer.querySelectorAll("[data-drawer-tab]").forEach((el) => {
          el.classList.toggle("is-on", el.getAttribute("data-drawer-tab") === name);
        });
      };

      const requested = params.get("drawer");
      if (requested) openDrawer(requested);
      else drawer.hidden = true;

      ccLab.querySelectorAll("[data-drawer-open]").forEach((el) => {
        el.addEventListener("click", (event) => {
          event.preventDefault();
          openDrawer(el.getAttribute("data-drawer-open"));
        });
      });
      ccLab.querySelectorAll("[data-drawer-tab]").forEach((el) => {
        el.addEventListener("click", (event) => {
          event.preventDefault();
          openDrawer(el.getAttribute("data-drawer-tab"));
        });
      });
      ccLab.querySelectorAll("[data-drawer-close]").forEach((el) => {
        el.addEventListener("click", (event) => {
          event.preventDefault();
          drawer.hidden = true;
        });
      });
    }
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
