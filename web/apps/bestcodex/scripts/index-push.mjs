/**
 * 主动把 URL 推给搜索引擎，缩短收录等待。
 *
 * 覆盖范围（这是现实边界，不是取舍）：
 * - IndexNow 一次提交同时到达 Bing、DuckDuckGo、Yandex、Seznam、Naver。
 * - 百度有独立的主动推送接口，需要在「百度搜索资源平台」拿站点专属 token。
 * - **Google 没有开放的推送端点**：Indexing API 只对招聘与直播类结构化数据开放，
 *   普通页面只能靠 sitemap + Search Console，本脚本对此无能为力。
 * - 搜狗、360 也没有公开推送接口，只能在各自站长平台提交 sitemap。
 *
 * 默认 dry-run，只打印将要提交的内容；确认无误后加 `--live` 真正提交。
 *
 * 用法：
 *   node scripts/index-push.mjs                      # 推 sitemap 里的全部 URL（dry-run）
 *   node scripts/index-push.mjs --live               # 真正提交
 *   node scripts/index-push.mjs --live /guides/xxx   # 只推指定路径
 *
 * 环境变量：
 *   INDEXNOW_KEY      IndexNow 密钥（同时要让构建产出 <key>.txt，见 prerender.mjs）
 *   BAIDU_PUSH_TOKEN  百度搜索资源平台的推送 token
 *   SITE_ORIGIN       站点源，默认 https://bestcodex.app
 */

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const distDir = join(here, "..", "dist");

const args = process.argv.slice(2);
const live = args.includes("--live");
const explicitPaths = args.filter((arg) => arg.startsWith("/"));

const ORIGIN = process.env.SITE_ORIGIN ?? "https://bestcodex.app";
const HOST = new URL(ORIGIN).host;
const INDEXNOW_KEY = process.env.INDEXNOW_KEY;
const BAIDU_TOKEN = process.env.BAIDU_PUSH_TOKEN;

async function urlsToPush() {
  if (explicitPaths.length) return explicitPaths.map((path) => `${ORIGIN}${path}`);

  const sitemap = await readFile(join(distDir, "sitemap.xml"), "utf8").catch(() => {
    throw new Error("读不到 dist/sitemap.xml，先跑 npm run build");
  });
  return [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => match[1]);
}

async function pushIndexNow(urls) {
  if (!INDEXNOW_KEY) {
    console.log("· IndexNow：跳过（未设 INDEXNOW_KEY）");
    return;
  }
  const payload = { host: HOST, key: INDEXNOW_KEY, urlList: urls };

  if (!live) {
    console.log(`· IndexNow：将提交 ${urls.length} 条到 api.indexnow.org（dry-run）`);
    return;
  }
  const response = await fetch("https://api.indexnow.org/indexnow", {
    method: "POST",
    headers: { "Content-Type": "application/json; charset=utf-8" },
    body: JSON.stringify(payload),
  });
  // IndexNow 用状态码表达结果：200 收到，202 收到但密钥待验证，422 URL 与 host 不符。
  console.log(`· IndexNow：HTTP ${response.status} ${response.statusText}`);
  if (response.status === 403) {
    console.log("  403 通常是根目录没有托管 <key>.txt，或内容与密钥不一致。");
  }
}

async function pushBaidu(urls) {
  if (!BAIDU_TOKEN) {
    console.log("· 百度：跳过（未设 BAIDU_PUSH_TOKEN）");
    return;
  }
  const endpoint = `http://data.zz.baidu.com/urls?site=${encodeURIComponent(ORIGIN)}&token=${BAIDU_TOKEN}`;

  if (!live) {
    console.log(`· 百度：将提交 ${urls.length} 条到 data.zz.baidu.com（dry-run）`);
    return;
  }
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: urls.join("\n"),
  });
  const body = await response.text();
  // 返回体含 success / remain（当日剩余配额）/ not_same_site 等字段。
  console.log(`· 百度：HTTP ${response.status} ${body}`);
}

async function main() {
  const urls = await urlsToPush();
  console.log(`${live ? "提交" : "dry-run"}：${urls.length} 条 URL，host=${HOST}`);
  for (const url of urls) console.log(`  ${url}`);
  console.log("");

  await pushIndexNow(urls);
  await pushBaidu(urls);

  console.log("");
  console.log("· Google：无开放推送端点，走 sitemap + Search Console。");
  console.log("· 搜狗 / 360：无公开推送接口，在各自站长平台提交 sitemap。");
  if (!live) console.log("\n以上为 dry-run。确认无误后加 --live 真正提交。");
}

await main();
