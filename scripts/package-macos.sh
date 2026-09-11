#!/bin/bash
# 把 release 二进制打包成标准 macOS .app：
#   target/release/bundle/ClipBeam.app
# 双击（或 open）启动，无终端窗口；LSUIElement=1，不占 Dock，仅菜单栏图标。
#
# 用法：
#   scripts/package-macos.sh              # 构建并打包
#   scripts/package-macos.sh --install    # 额外拷贝到 /Applications
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')"
APP_NAME="ClipBeam"
BIN_NAME="clipbeam"
BUNDLE_ID="com.clipbeam.app"
PROFILE="release"
BUNDLE_DIR="target/${PROFILE}/bundle"
APP="${BUNDLE_DIR}/${APP_NAME}.app"

echo "==> cargo build --release"
cargo build --release

echo "==> 定位构建期生成的 1024px 图标"
ICON_PNG="$(ls -t target/${PROFILE}/build/clipbeam-*/out/clipbeam_icon_1024.png 2>/dev/null | head -1 || true)"
if [[ -z "${ICON_PNG}" || ! -f "${ICON_PNG}" ]]; then
  echo "找不到 clipbeam_icon_1024.png（build.rs 产物），请先 cargo build --release" >&2
  exit 1
fi

echo "==> 组装 ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"
cp "target/${PROFILE}/${BIN_NAME}" "${APP}/Contents/MacOS/${BIN_NAME}"
chmod +x "${APP}/Contents/MacOS/${BIN_NAME}"
printf 'APPL????' > "${APP}/Contents/PkgInfo"

# ---- Info.plist（LSUIElement=1 → 无 Dock、纯菜单栏应用）----
cat > "${APP}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>${BIN_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>${BIN_NAME}.icns</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>ClipBeam</string>
</dict>
</plist>
PLIST

# ---- .icns（sips 切全尺寸 + iconutil 合成）----
ICONSET="$(mktemp -d)/${BIN_NAME}.iconset"
mkdir -p "${ICONSET}"
gen() { sips -z "$2" "$2" "${ICON_PNG}" --out "${ICONSET}/$1" >/dev/null; }
gen icon_16x16.png        16
gen icon_16x16@2x.png     32
gen icon_32x32.png        32
gen icon_32x32@2x.png     64
gen icon_128x128.png      128
gen icon_128x128@2x.png   256
gen icon_256x256.png      256
gen icon_256x256@2x.png   512
gen icon_512x512.png      512
gen icon_512x512@2x.png   1024
iconutil -c icns "${ICONSET}" -o "${APP}/Contents/Resources/${BIN_NAME}.icns"

echo "==> ad-hoc 代码签名（本地使用，TCC 权限需要稳定签名）"
codesign --force --sign - "${APP}"

echo "==> 向 LaunchServices 注册"
LSREG="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
"${LSREG}" "${APP}" >/dev/null 2>&1 || true

echo
echo "✓ 打包完成：${APP}"
if [[ "${1:-}" == "--install" ]]; then
  echo "==> 安装到 /Applications"
  rm -rf "/Applications/${APP_NAME}.app"
  cp -R "${APP}" "/Applications/${APP_NAME}.app"
  echo "✓ 已安装 /Applications/${APP_NAME}.app（Spotlight 搜索 ClipBeam 或 Launchpad 启动）"
else
  cat <<TIP

使用方法：
  open "${APP}"          # 或在 Finder 中双击
  cp -R "${APP}" /Applications/   # 可选：安装到应用程序目录

首次启动请到「系统设置 → 隐私与安全性」授予：辅助功能、屏幕录制、通知，
授权后重新启动 ClipBeam。
TIP
fi
