/**
 * 指南的英文版，slug 与中文版一一对应——`seo.ts` 靠相同 slug 配 hreflang 互指。
 *
 * 这是翻译而非另写：事实、口径、保守措辞都必须与 `guides.ts` 一致。尤其是封号那篇，
 * 英文同样只说降低风险，不说保证。改中文版时这里要跟着改，否则两种语言给出不同承诺。
 */

import type { Guide } from "./guides";

export const GUIDES_EN: Guide[] = [
  {
    slug: "claude-code-ban",
    question: "Can Claude Code get your account banned, and how do you reduce the risk?",
    title: "Claude Code ban risk, and how to lower it",
    summary:
      "Ban risk comes mostly from your environment: shared egress IPs, a network that jumps around. A dedicated server lowers exposure; nothing can guarantee it.",
    answer:
      "Ban risk comes largely from your **runtime environment** rather than the code you write: a shared egress IP, a network location that jumps around, and coding sessions mixed in with everything else on your machine all make an account look anomalous. Running the official Claude Code on a dedicated server — stable IP, isolated environment, separated from your local activity — meaningfully reduces that exposure. To be clear: **no approach can guarantee you won't be banned.** The terms of service and the risk rules belong to Anthropic; any third party can only reduce risk, not remove it.",
    sections: [
      {
        heading: "Why the environment matters more than usage",
        body: [
          "One account appearing from several geographic locations and several egress IPs in a short window is the classic anomaly signal. When you use a public proxy or hop between nodes, the address you exit from carries the history of everyone else who used it before you.",
          "Your local machine adds noise of its own: the browser, other tools, and other accounts all share one network exit. Move the coding session into a stable environment of its own and the account's behavioural trail gets a lot cleaner.",
        ],
      },
      {
        heading: "What a dedicated server actually fixes",
        body: [
          "A stable IP: the address you exit from stays the same instead of changing daily.",
          "An isolated environment: Claude Code runs on a machine that does nothing else, separated from your local activity.",
          "Persistent sessions: when the connection drops the session is still alive on the server, so the conversation context survives and you don't start over.",
        ],
      },
      {
        heading: "Where your code lives, and what we can see",
        body: [
          "Files sync only between your machine and your server. We store account and workspace metadata; we do not store your source code.",
          "Secret files are excluded from sync by default. When both sides changed the same file you get a side-by-side comparison and make the call — nothing is silently overwritten.",
        ],
      },
      {
        heading: "What BestCodex does here",
        body: [
          "The Claude tab in BestCodex is this setup, packaged: download a launcher, sign in once, and it brings the official Claude Code up on your server and handles sync and conflicts. The first run explains itself before connecting rather than opening with a long SSH form.",
          "Workspaces are unlimited and usage is drawn from your balance. Signing up through a friend's invite link and signing in to the app gets you a free month, once per account.",
          "BestCodex is an independent project, not affiliated with, sponsored by, or endorsed by OpenAI or Anthropic. Confirm for yourself that your usage complies with the relevant terms of service.",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "claude-code-on-your-server",
    question: "How do you run Claude Code on your own server?",
    title: "Running Claude Code on your own server: three problems to solve",
    summary:
      "Three problems to solve: a stable runtime, a session that survives a dropped connection, and reliable two-way file sync with real conflict handling.",
    answer:
      "Running the official Claude Code on your own server comes down to three problems: a stable runtime environment (fixed IP, dependencies installed), a persistent session that doesn't lose context when the connection drops, and reliable two-way file sync between the server and your machine. The hand-rolled version is usually SSH plus a process supervisor plus a sync tool — workable, but the maintenance cost is real, especially around conflict handling and excluding secret files. The Claude tab in BestCodex packages all three so they work out of the box: sign in once, and it handles the environment, the session, and the sync.",
    sections: [
      {
        heading: "What you have to handle yourself",
        body: [
          "Environment: install dependencies, pin the egress IP, keep the machine online.",
          "Session: the process has to survive SSH disconnecting, and the next connection has to resume the conversation rather than start blank.",
          "Sync: remote edits need to reach your local editor, local edits need to reach the server, and simultaneous edits on both sides need a way to converge.",
          "Safety: `.env` files, keys, and credentials must not get swept up into the sync.",
        ],
      },
      {
        heading: "The hard part is conflicts, not transfer",
        body: [
          "Moving files is easy. The hard part is what happens when both sides changed the same file. Silent overwriting is the worst possible answer — you find out weeks later that an edit vanished.",
          "BestCodex shows a side-by-side comparison and lets you decide instead of choosing for you. Secret files are excluded by default so credentials don't get pushed to the remote.",
        ],
      },
      {
        heading: "The BestCodex path",
        body: [
          "Download the launcher (macOS 13+ on Apple silicon or Intel, or Windows 10/11 64-bit), sign in inside the app, and switch to the Claude tab.",
          "The first run explains itself before connecting. After that workspaces are unlimited and terminals are persistent — close the window, come back, and the session is still there.",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "codex-zero-config",
    question: "Codex setup is tedious — is there a zero-configuration way?",
    title: "Using the official Codex with zero configuration",
    summary:
      "No base URL, no API key. Sign in once and the local config is written for you — what launches is the official Codex app itself.",
    answer:
      "If you're stuck filling in a base URL, pasting an API key, and editing config files, you can let a launcher do it: download BestCodex, sign in once inside the app, and it writes the local connection config. Clicking **Launch Codex** then opens the **official Codex app itself**. BestCodex is not the official Codex and does not modify it — it writes only the connection config it manages, takes a backup before writing, and can restore the pre-takeover state at any time.",
    sections: [
      {
        heading: "Three steps",
        body: [
          "One: download and install. Pick your platform — Mac with Apple silicon, Mac with Intel, or Windows. The official Codex app is **not bundled**; install it separately when you need it.",
          "Two: sign in inside the app. The connection and local config are written automatically. There is no service address or key to enter.",
          "Three: launch the official Codex. Balance and top-ups happen in your account.",
        ],
      },
      {
        heading: "One-line install",
        body: [
          "macOS: `curl -fsSL https://bestcodex.app/install.sh | sh`",
          "Windows (PowerShell): `irm https://bestcodex.app/install.ps1 | iex`",
          "The script picks the build for your chip, verifies it against `SHA256SUMS.txt`, installs into Applications, and clears the quarantine attribute for you — so you don't have to run `xattr -cr` by hand. No sudo, and it writes no configuration.",
          "You still sign in inside the app afterwards; the command line only puts the app in place.",
        ],
      },
      {
        heading: "What it changes on your machine",
        body: [
          "Only the connection config it manages, and it backs that up before writing. If existing local config conflicts with what needs to be written, the launcher takes you to a repair screen for confirmation rather than overwriting in place.",
          "You can restore the pre-takeover snapshot at any time. The official app itself is left untouched.",
        ],
      },
      {
        heading: "If the OS blocks the first launch",
        body: [
          "These are unsigned beta builds. macOS reports “damaged and can't be opened” because of the quarantine attribute — the app is fine. See [macOS says the app is damaged](/en/guides/macos-damaged-app).",
          "On Windows, SmartScreen will warn you; verify the source and continue.",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "macos-damaged-app",
    question: "macOS says the app is damaged and can't be opened — how do you fix it?",
    title: "macOS: “damaged and can't be opened”",
    summary:
      "Gatekeeper blocking an unsigned app via the quarantine attribute, not a bad download. Move it to Applications and clear the attribute with xattr -cr.",
    answer:
      "The app isn't damaged — macOS Gatekeeper is blocking it. An unsigned app downloaded through a browser gets tagged with a quarantine attribute, and the system reports that as “damaged and can't be opened”. Move the app to Applications, then run `xattr -cr \"/Applications/BestCodex.app\"` to clear the attribute and open it again. If it still won't open, Control-click the icon and choose **Open** to confirm explicitly.",
    sections: [
      {
        heading: "Why this happens",
        body: [
          "Gatekeeper checks an app's signature and notarization status. Beta builds are not yet signed or notarized, so they're treated as untrusted — and surfaced with the misleading word “damaged”.",
          "You'll see the same message on many unsigned open-source apps. It isn't specific to this one.",
        ],
      },
      {
        heading: "Using the command",
        body: [
          "Move the app to Applications first, then run: `xattr -cr \"/Applications/BestCodex.app\"`.",
          "`-c` clears extended attributes and `-r` recurses through everything inside the bundle. The path has to match the actual app name.",
          "Don't run it across your whole Applications folder, and you don't need sudo.",
          "To skip this step entirely, install with `curl -fsSL https://bestcodex.app/install.sh | sh`: the script verifies the SHA256, installs into Applications, and clears the quarantine attribute for you.",
        ],
      },
      {
        heading: "Still won't open",
        body: [
          "Control-click the icon, choose **Open**, then confirm **Open** again in the dialog.",
          "If the error mentions permissions or architecture instead, check that you downloaded the build for your chip — Apple silicon and Intel are separate installers.",
        ],
      },
    ],
    updated: "2026-08-17",
  },
  {
    slug: "vs-codex-plus-plus",
    question: "What's the difference between BestCodex and Codex++, and which should you use?",
    title: "BestCodex compared with Codex++",
    summary:
      "Same codebase, different goals: Codex++ is for deep customization, BestCodex is for working out of the box plus Claude Code on your own server.",
    answer:
      "They share a codebase: the BestCodex desktop app is an AGPL-3.0 fork of [Codex++](https://github.com/BigPizzaV3/CodexPlusPlus). The goals differ, though. Codex++ is for advanced users who want to deeply modify Codex — its strengths are provider switching, protocol translation, plugin unlocking, and UI enhancements. BestCodex is for people who want the official Codex working quickly — its strengths are zero-config setup and account/balance handling, plus a Claude tab that runs the official Claude Code on your own server. **If you want provider switching or deep customization, use upstream Codex++ directly.**",
    sections: [
      {
        heading: "Pick by what you need",
        body: [
          "Want to point Codex at DeepSeek, Claude, or another custom provider, unlock plugin entry points in API-key mode, or manage sessions in bulk? Use Codex++.",
          "Want minimal fuss — sign in once and use the official Codex — plus Claude Code on a dedicated server? Use BestCodex.",
          "Neither modifies the installed files of the official Codex app.",
        ],
      },
      {
        heading: "What sharing a codebase means",
        body: [
          "AGPL-3.0 is a copyleft license: distributing a modified version requires providing source under the same license. BestCodex preserves upstream attribution and license notices, and the repository shows its fork origin on GitHub.",
          "Upstream's feature work and BestCodex's choices will keep diverging. Don't assume the two match feature for feature.",
        ],
      },
      {
        heading: "Cost",
        body: [
          "Codex++ is free and open source. BestCodex's Claude capability is usage-based — ¥19.9 is a reference top-up amount, not an auto-renewing monthly plan — and the Codex side draws on your account balance.",
          "Signing up through a friend's invite link and signing in to the app gets you a free month, once per account.",
        ],
      },
    ],
    updated: "2026-08-17",
  },
];

export function guideEnBySlug(slug: string | undefined): Guide | undefined {
  if (!slug) return undefined;
  return GUIDES_EN.find((guide) => guide.slug === slug);
}
