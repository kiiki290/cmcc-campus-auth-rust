#!/usr/bin/env bash
# 下载 Chrome for Testing 的 chrome-headless-shell 到指定目录。
# 用法：scripts/fetch_chromium.sh <目标目录>
set -euo pipefail

DEST="${1:?用法: $0 <目标目录>}"
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) PLATFORM=linux64 ;;
    Linux-aarch64 | Linux-arm64) PLATFORM=linux-arm64 ;;
    *)
        echo "不支持的平台：$(uname -s)-$(uname -m)" >&2
        exit 1
        ;;
esac

command -v jq >/dev/null || { echo "需要 jq 解析 Chrome for Testing 清单" >&2; exit 1; }
command -v unzip >/dev/null || { echo "需要 unzip" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

MANIFEST="https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json"
curl -fsSL "$MANIFEST" -o "$TMP/manifest.json"
VERSION="$(jq -r '.channels.Stable.version' "$TMP/manifest.json")"
URL="$(jq -r --arg p "$PLATFORM" '
    .channels.Stable.downloads["chrome-headless-shell"][]
    | select(.platform == $p) | .url' "$TMP/manifest.json")"
[ -n "$URL" ] || { echo "清单中没有 $PLATFORM 的 chrome-headless-shell" >&2; exit 1; }

# 缓存 zip，重复打包不再下载约 200MB
CACHE_ROOT="${CMCC_CHROMIUM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/cmcc-campus-auth/chrome-for-testing}"
mkdir -p "$CACHE_ROOT"
CACHED_ZIP="$CACHE_ROOT/chrome-headless-shell-${VERSION}-${PLATFORM}.zip"
if [ ! -s "$CACHED_ZIP" ] || ! unzip -tq "$CACHED_ZIP" >/dev/null 2>&1; then
    echo "下载：$URL"
    rm -f "$CACHED_ZIP"
    curl -fL --retry 3 -o "$CACHED_ZIP.part" "$URL"
    mv "$CACHED_ZIP.part" "$CACHED_ZIP"
else
    echo "使用缓存：$CACHED_ZIP"
fi
unzip -q "$CACHED_ZIP" -d "$TMP"

DIR_NAME="$(basename "${URL%.zip}")"
[ -d "$TMP/$DIR_NAME" ] || { echo "压缩包结构异常：缺少 $DIR_NAME" >&2; exit 1; }

mkdir -p "$DEST"
rm -rf "${DEST:?}/$DIR_NAME"
cp -a "$TMP/$DIR_NAME" "$DEST/"
chmod +x "$DEST/$DIR_NAME/chrome-headless-shell"
echo "已放置：$DEST/$DIR_NAME/chrome-headless-shell"
