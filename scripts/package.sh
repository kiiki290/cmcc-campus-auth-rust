#!/usr/bin/env bash
# 构建两个 Linux 分发包：
#   - lite：只含程序，使用系统 Chrome/Edge/Chromium
#   - bundled：额外包含 chrome-headless-shell（约 200MB）
#
# 用法：scripts/package.sh [all|lite|bundled]
set -euo pipefail

cd "$(dirname "$0")/.."
MODE="${1:-all}"
case "$MODE" in all | lite | bundled) ;; *) echo "用法：$0 [all|lite|bundled]" >&2; exit 1 ;; esac

command -v jq >/dev/null || { echo "需要 jq" >&2; exit 1; }
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')"
ARCH="$(uname -m)"
BIN="target/release/cmcc-campus-auth"

echo "构建 release（v$VERSION）..."
cargo build --release

rm -rf dist
mkdir -p dist
COMMON=(README.md LICENSE THIRD_PARTY_NOTICES.md)

if [ "$MODE" = all ] || [ "$MODE" = lite ]; then
    NAME="cmcc-campus-auth-${VERSION}-linux-${ARCH}-lite"
    mkdir -p "dist/$NAME"
    cp "$BIN" "dist/$NAME/"
    cp "${COMMON[@]}" "dist/$NAME/"
    mkdir -p "dist/$NAME/docs"
    cp docs/screenshot.png "dist/$NAME/docs/"
    tar -C dist -czf "dist/${NAME}.tar.gz" "$NAME"
    echo "已生成：dist/${NAME}.tar.gz"
fi

if [ "$MODE" = all ] || [ "$MODE" = bundled ]; then
    NAME="cmcc-campus-auth-${VERSION}-linux-${ARCH}-bundled"
    mkdir -p "dist/$NAME"
    cp "$BIN" "dist/$NAME/"
    cp "${COMMON[@]}" "dist/$NAME/"
    mkdir -p "dist/$NAME/docs"
    cp docs/screenshot.png "dist/$NAME/docs/"
    scripts/fetch_chromium.sh "dist/$NAME/chromium"
    tar -C dist -czf "dist/${NAME}.tar.gz" "$NAME"
    echo "已生成：dist/${NAME}.tar.gz"
fi
