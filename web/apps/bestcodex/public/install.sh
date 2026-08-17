#!/bin/sh
#
# BestCodex installer for macOS — https://bestcodex.app
#
#   curl -fsSL https://bestcodex.app/install.sh | sh
#
# 只做四件事：按芯片挑安装包、按 SHA256 校验、装进「应用程序」、清掉隔离标记。
# 不写任何配置、不要 sudo、不留后台进程。装完的应用与手动拖进「应用程序」的完全一样。
#
# 之所以需要清隔离标记：内测包尚未签名公证，Gatekeeper 会把它报成「已损坏」。
# 这一步等价于手册里让你敲的 `xattr -cr`，脚本只是替你敲，不改任何系统设置。
#
# 环境变量：
#   BESTCODEX_MANIFEST_URL  覆盖版本指针（默认 S3）
#   BESTCODEX_PREFIX        安装目录（默认 /Applications）
#   BESTCODEX_DRY_RUN=1     只解析并打印将要做什么，不下载不安装

set -eu

MANIFEST_URL="${BESTCODEX_MANIFEST_URL:-https://s3.lumio.games/lumio-codex/releases/latest-internal.json}"
PREFIX="${BESTCODEX_PREFIX:-/Applications}"
APP_NAME="BestCodex.app"
DRY_RUN="${BESTCODEX_DRY_RUN:-0}"

WORK_DIR=""
MOUNT_POINT=""

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

cleanup() {
  # 卸载顺序要紧：先弹镜像再删临时目录，否则挂载点被占用会留下僵尸卷。
  if [ -n "$MOUNT_POINT" ] && [ -d "$MOUNT_POINT" ]; then
    hdiutil detach "$MOUNT_POINT" -quiet >/dev/null 2>&1 || true
  fi
  if [ -n "$WORK_DIR" ] && [ -d "$WORK_DIR" ]; then
    rm -rf "$WORK_DIR"
  fi
}
trap cleanup EXIT INT TERM

[ "$(uname -s)" = "Darwin" ] || die "这个脚本只装 macOS 版。Windows 用 PowerShell：irm https://bestcodex.app/install.ps1 | iex"

command -v curl >/dev/null 2>&1 || die "找不到 curl"
command -v hdiutil >/dev/null 2>&1 || die "找不到 hdiutil，这不像是 macOS"

# Rosetta 下 uname -m 会谎报 x86_64，导致给 Apple 芯片装 Intel 包。
arch="$(uname -m)"
if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
  arch="arm64"
fi

case "$arch" in
  arm64) slug="macos-arm64" ;;
  x86_64) slug="macos-x64" ;;
  *) die "不认识的芯片架构：$arch" ;;
esac

say "→ 读取版本指针 $MANIFEST_URL"
manifest="$(curl -fsSL --max-time 30 "$MANIFEST_URL")" || die "读不到版本指针，检查网络后重试"

# 指针是构建产出的机器生成 JSON，格式可能是紧凑或缩进的。不依赖换行位置，直接抓 URL。
asset_url="$(printf '%s' "$manifest" \
  | tr ',' '\n' \
  | grep -o "https://[^\"]*${slug}-internal-unsigned\.dmg" \
  | head -n 1)"
[ -n "$asset_url" ] || die "指针里没有 $slug 的安装包"

version="$(printf '%s' "$manifest" | tr ',' '\n' | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
dmg_name="${asset_url##*/}"
sums_url="${asset_url%/*}/SHA256SUMS.txt"

say "→ 版本 ${version:-未知}（内测版，未签名）"
say "→ 安装包 $dmg_name"
say "→ 装到 $PREFIX/$APP_NAME"

if [ "$DRY_RUN" = "1" ]; then
  say "（BESTCODEX_DRY_RUN=1，到此为止，什么都没动）"
  exit 0
fi

[ -d "$PREFIX" ] || die "$PREFIX 不存在"
[ -w "$PREFIX" ] || die "$PREFIX 不可写。换个位置：BESTCODEX_PREFIX=\"\$HOME/Applications\""

WORK_DIR="$(mktemp -d)"
dmg_path="$WORK_DIR/$dmg_name"

say "→ 下载"
curl -fL --progress-bar --max-time 900 -o "$dmg_path" "$asset_url" || die "下载失败"

say "→ 校验 SHA256"
# 校验失败一律中止：宁可不装，也不能把来源不明的二进制放进「应用程序」。
expected="$(curl -fsSL --max-time 30 "$sums_url" 2>/dev/null | grep " $dmg_name\$" | awk '{print $1}' || true)"
[ -n "$expected" ] || die "取不到校验和（$sums_url），中止"
actual="$(shasum -a 256 "$dmg_path" | awk '{print $1}')"
[ "$expected" = "$actual" ] || die "校验和不匹配，中止。期望 $expected，实际 $actual"

say "→ 挂载镜像"
MOUNT_POINT="$WORK_DIR/mnt"
mkdir -p "$MOUNT_POINT"
hdiutil attach "$dmg_path" -mountpoint "$MOUNT_POINT" -nobrowse -readonly -quiet \
  || die "挂载失败"

src="$MOUNT_POINT/$APP_NAME"
[ -d "$src" ] || src="$(find "$MOUNT_POINT" -maxdepth 1 -name '*.app' -print -quit)"
[ -n "$src" ] && [ -d "$src" ] || die "镜像里找不到应用"

target="$PREFIX/$APP_NAME"
if [ -e "$target" ]; then
  say "→ 覆盖已有的 $target"
  rm -rf "$target"
fi

say "→ 拷贝到 $PREFIX"
# ditto 会保留 bundle 里的符号链接与扩展属性，cp -R 不保证。
ditto "$src" "$target" || die "拷贝失败"

say "→ 清除隔离标记（未签名包必需，等价于手动执行 xattr -cr）"
xattr -cr "$target" || die "清除隔离标记失败，手动执行：xattr -cr \"$target\""

say ""
say "装好了：$target"
say "打开它，在应用内登录一次，连接与本机配置会自动写好。"
say "官方 Codex 应用不捆绑在这里，需要时另行安装。"
say "遇到问题看 https://bestcodex.app/help/unsigned"
