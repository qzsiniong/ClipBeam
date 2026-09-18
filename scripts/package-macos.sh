#!/bin/bash
# 用 Tauri 2 构建并打包成标准 macOS .app：
#   src-tauri/target/release/bundle/macos/ClipBeam.app
# 双击（或 open）启动，无终端窗口；LSUIElement=1（tauri.conf.json 配置），不占 Dock。
#
# 用法：
#   scripts/package-macos.sh              # 构建并打包
#   scripts/package-macos.sh --install    # 额外拷贝到 /Applications
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

APP_NAME="ClipBeam"
SRC_TAURI_DIR="src-tauri"

# 检查 pnpm 与 frontend 依赖
if ! command -v pnpm &>/dev/null; then
  echo "✗ 未找到 pnpm，请先安装：npm i -g pnpm" >&2
  exit 1
fi

if [[ ! -d "node_modules" ]]; then
  echo "==> 安装 frontend 依赖"
  (pnpm install)
fi

# 检查 tauri-cli（cargo 子命令）
if ! cargo tauri --version &>/dev/null; then
  echo "==> 安装 tauri-cli"
  cargo install tauri-cli --version "^2.0.0" --locked
fi

echo "==> cargo tauri build（Tauri 2 自动：前端构建 + Rust release + bundle）"
(cd "$SRC_TAURI_DIR" && cargo tauri build)

# Tauri 2 输出路径
BUNDLE_APP="${REPO}/${SRC_TAURI_DIR}/target/release/bundle/macos/${APP_NAME}.app"

if [[ ! -d "${BUNDLE_APP}" ]]; then
  echo "✗ 打包产物未找到：${BUNDLE_APP}" >&2
  exit 1
fi

echo "==> ad-hoc 代码签名（本地使用，TCC 权限需要稳定签名）"
codesign --force --sign - "${BUNDLE_APP}"

echo "==> 向 LaunchServices 注册"
LSREG="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
"${LSREG}" "${BUNDLE_APP}" >/dev/null 2>&1 || true

echo
echo "✓ 打包完成：${BUNDLE_APP}"
if [[ "${1:-}" == "--install" ]]; then
  echo "==> 安装到 /Applications"
  rm -rf "/Applications/${APP_NAME}.app"
  cp -R "${BUNDLE_APP}" "/Applications/${APP_NAME}.app"
  echo "✓ 已安装 /Applications/${APP_NAME}.app（Spotlight 搜索 ClipBeam 或 Launchpad 启动）"
else
  cat <<TIP

使用方法：
  open "${BUNDLE_APP}"          # 或在 Finder 中双击
  cp -R "${BUNDLE_APP}" /Applications/   # 可选：安装到应用程序目录

首次启动请到「系统设置 → 隐私与安全性」授予：辅助功能、屏幕录制、通知，
授权后重新启动 ClipBeam。
TIP
fi
