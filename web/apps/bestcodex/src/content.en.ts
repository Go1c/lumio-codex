/**
 * 落地页英文文案。翻译而非另写：价格、承诺、免责声明必须与 `content.ts` 一致。
 * 封号只说降低风险，不说保证。改中文口径时这里要跟着改。
 */

import { PLAN } from "@/content";

export const TERMINAL_SHOT_EN = [
  "BestCodex · Claude · my-project",
  "attached  session my-project  ·  terminal",
  "",
  "> claude",
  "Claude Code · my-project",
  "How can I help you today?",
  "",
  "> _",
];

export const VALUES_EN = [
  {
    title: "Lower the ban risk",
    body: "A dedicated server, a stable IP, and a persistent session — isolated from your machine. That cuts the usual anomaly signals. If the connection drops, the conversation context is still there.",
  },
  {
    title: "Two-way sync, no silent overwrite",
    body: "Remote edits come back to your machine; local edits go up to the server. Secret files stay out of sync by default. Conflicts are shown side by side — nothing is overwritten for you.",
  },
];

export const PLAN_EN: typeof PLAN = {
  tag: "Usage-based",
  name: "Claude top-up",
  price: "¥19.9",
  per: "reference amount · not an auto-renewing monthly plan",
  features: [
    "Dedicated server environment",
    "Two-way sync with conflict review",
    "Persistent terminal",
    "Unlimited workspaces",
  ],
  noLimits: "No plan walls. Usage is drawn from your balance.",
  inviteLine: "🎁 Sign up through a friend's invite and sign in to the app",
  inviteOnce: "First month free (once per account)",
};

const MACOS_DAMAGED_FAQ_EN: [string, string] = [
  "macOS says the app is damaged and can't be opened — what now?",
  "Gatekeeper is blocking an unsigned beta build; the app is not actually damaged. Move BestCodex to Applications, then run: xattr -cr \"/Applications/BestCodex.app\". If it still won't open, Control-click the icon and choose Open.",
];

export const CLAUDE_FAQS_EN: Array<[string, string]> = [
  [
    "Is there a free plan?",
    "No free plan. Sign up through a friend's invite link and sign in, and you get one free month with the same features as a paying account.",
  ],
  [
    "Are there usage limits?",
    "No. Unlimited workspaces, unlimited sync, every feature on. You pay from your balance.",
  ],
  [
    "Do you store my source code?",
    "Files sync only between your machine and your server. We store account and workspace metadata, not your source.",
  ],
  [
    "Is this an auto-renewing subscription?",
    "No. The button opens a top-up page. You use what you put in. There is no auto-renewing monthly contract.",
  ],
  MACOS_DAMAGED_FAQ_EN,
];

export const CODEX_FAQS_EN: Array<[string, string]> = [
  [
    "Is BestCodex the official Codex?",
    "No. It is an independent launcher that gets you onto the official Codex faster. After it writes the connection config, what you use day to day is the official Codex app itself.",
  ],
  [
    "Does it modify the official Codex app?",
    "No. The official app is left untouched. BestCodex only writes the connection config it manages, takes a backup first, and can restore the pre-takeover state at any time.",
  ],
  MACOS_DAMAGED_FAQ_EN,
];

export const CODEX_STEPS_EN = [
  {
    num: "01",
    title: "Download and install",
    body: "Pick your platform. The official Codex app is installed separately — this tool does not bundle or modify it.",
  },
  {
    num: "02",
    title: "Sign in inside the app",
    body: "Signing in writes the connection and local config. No service address or key to fill in.",
  },
  {
    num: "03",
    title: "Launch official Codex",
    body: "Click Launch Codex to open the official app. Balance and top-ups live in your account.",
  },
];

export const CODEX_HERO_EN = {
  kicker: "Official app",
  titleLead: "Start using",
  titleEm: "official Codex",
  sub: "We handle sign-up, sign-in, and the local config. What you run is always the official Codex app — not bundled, not modified.",
  download: "Download BestCodex",
  faq: "FAQ",
  startKicker: "Get started",
  startTitle: "Three steps",
};

export const CLAUDE_HERO_EN = {
  kicker: "Built to lower ban risk",
  titleLead: "Use Claude Code",
  titleEm: "without the usual exposure",
  sub: "Official Claude Code on your own server — isolated environment, stable IP, persistent sessions. Files sync both ways; it feels like working locally.",
  download: "Download BestCodex",
  pricing: "See pricing",
  whyKicker: "Why",
  whyTitle: "Risk, and sync",
  priceKicker: "Pricing",
  priceTitle: "Simple pricing",
  termLabel: "Claude terminal",
};
