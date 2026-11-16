#!/usr/bin/env bash
# Build a universal (arm64 + x86_64) Lumora.app, sign it, optionally
# notarize, and package a DMG.
#
#   scripts/bundle.sh                         # ad-hoc signed, no notarization
#   SIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
#   NOTARY_PROFILE=lumora-notary scripts/bundle.sh
#
# NOTARY_PROFILE is a keychain profile created with
#   xcrun notarytool store-credentials lumora-notary --apple-id … --team-id …
#
# Note: ad-hoc signed builds get a new code identity every build, so macOS
# asks for Screen Recording permission again after each rebuild.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
DIST="$ROOT/dist"
APP="$DIST/Lumora.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
# CFBundleShortVersionString must be 1–3 dot-separated integers, so drop any
# semver pre-release / build suffix ("0.5.0-beta.1" → "0.5.0") for the plist;
# DMG / zip names keep the full $VERSION.
SHORT_VERSION="${VERSION%%[-+]*}"
BUILD="${BUILD_NUMBER:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

echo "==> Lumora $VERSION (CFBundleShortVersionString $SHORT_VERSION, build $BUILD)"
for t in "${TARGETS[@]}"; do
  rustup target add "$t" >/dev/null 2>&1 || true
  echo "==> cargo build --release --target $t"
  cargo build --release --locked -p lumora --target "$t"
done

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/lumora" \
  "target/aarch64-apple-darwin/release/lumora" \
  "target/x86_64-apple-darwin/release/lumora"
lipo -info "$APP/Contents/MacOS/lumora"
sed -e "s/__VERSION__/$SHORT_VERSION/" -e "s/__BUILD__/$BUILD/" packaging/Info.plist > "$APP/Contents/Info.plist"
# App icon: packaging/AppIcon.icns is generated (and committed) by
# scripts/make_icon.py from the SVG. With ICONUTIL=1 on a Mac, rebuild it
# here through Apple's iconutil from the same PNGs instead.
if [[ "${ICONUTIL:-0}" == "1" ]] && command -v iconutil >/dev/null 2>&1 && command -v python3 >/dev/null 2>&1; then
  ICONSET="$(mktemp -d)/AppIcon.iconset"
  python3 scripts/make_icon.py --iconset "$ICONSET"
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
else
  cp packaging/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
fi
printf 'APPL????' > "$APP/Contents/PkgInfo"
plutil -lint "$APP/Contents/Info.plist"

echo "==> codesign ($SIGN_IDENTITY)"
# Secure timestamps need Apple's server and a real identity; skip for ad-hoc.
TIMESTAMP="--timestamp"
[[ "$SIGN_IDENTITY" == "-" ]] && TIMESTAMP="--timestamp=none"
codesign --force --options runtime "$TIMESTAMP" --sign "$SIGN_IDENTITY" \
  --entitlements packaging/entitlements.plist "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  echo "==> Notarizing"
  ZIP="$DIST/Lumora-$VERSION.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  rm -f "$ZIP"
fi

DMG="$DIST/Lumora-$VERSION.dmg"
rm -f "$DMG"
echo "==> Creating $DMG"
if command -v create-dmg >/dev/null 2>&1; then
  create-dmg \
    --volname "Lumora $VERSION" \
    --window-size 540 360 \
    --icon-size 112 \
    --icon "Lumora.app" 140 170 \
    --app-drop-link 400 170 \
    --no-internet-enable \
    "$DMG" "$APP"
else
  echo "   (create-dmg not found; falling back to hdiutil)"
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "Lumora $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
  rm -rf "$STAGE"
fi
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
  [[ -n "${NOTARY_PROFILE:-}" ]] && xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait && xcrun stapler staple "$DMG"
fi
echo "==> Done: $DMG"
