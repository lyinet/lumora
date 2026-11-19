#!/usr/bin/env bash
# Build a universal (arm64 + x86_64) Lumora.app, sign it, optionally
# notarize, and package a DMG.
#
#   scripts/bundle.sh                         # ad-hoc signed, no notarization
#   SIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
#   NOTARY_PROFILE=lumora-notary scripts/bundle.sh
#
# NOTARY_PROFILE is a notarytool keychain profile created with
#   xcrun notarytool store-credentials lumora-notary --apple-id … --team-id …
# (or --key/--key-id/--issuer for an App Store Connect API key).
# NOTARY_KEYCHAIN optionally names the keychain that holds the profile
# (CI uses a temporary one, see scripts/sign-and-notarize.sh).
# APPLE_TEAM_ID, when set with a real identity, is checked against the
# signature's designated requirement.
#
# The code identity is pinned to the bundle identifier com.lumora.app. With a
# Developer ID certificate the designated requirement is then
#   identifier "com.lumora.app" and anchor apple generic and
#   certificate leaf[subject.OU] = TEAMID …
# which is the same for every build, so macOS keeps Screen Recording,
# Microphone, Accessibility and Keychain grants across updates.
#
# Note: ad-hoc signed builds get a new code identity every build (their
# designated requirement is the code hash), so macOS asks for Screen
# Recording permission again after each rebuild.
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
BUNDLE_ID="com.lumora.app"
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
# The bundle identifier is the code identity TCC and the Keychain remember,
# and hardened-runtime apps are killed when they touch a protected resource
# without its usage string, so refuse to ship a plist that lost either.
plist_value() { /usr/libexec/PlistBuddy -c "Print :$1" "$APP/Contents/Info.plist" 2>/dev/null || true; }
if [[ "$(plist_value CFBundleIdentifier)" != "$BUNDLE_ID" ]]; then
  echo "error: CFBundleIdentifier must stay $BUNDLE_ID (permissions are tied to it)" >&2
  exit 1
fi
for key in NSScreenCaptureUsageDescription NSMicrophoneUsageDescription NSSpeechRecognitionUsageDescription; do
  if [[ -z "$(plist_value "$key")" ]]; then
    echo "error: Info.plist is missing $key" >&2
    exit 1
  fi
done

# notarytool submit --wait exits 0 for an "Invalid" result on some Xcode
# versions, so read the status from its JSON and print Apple's log on failure.
notarize() {
  local file="$1" out status id
  local args=(--keychain-profile "$NOTARY_PROFILE")
  [[ -n "${NOTARY_KEYCHAIN:-}" ]] && args+=(--keychain "$NOTARY_KEYCHAIN")
  echo "==> Notarizing $(basename "$file")"
  out="$(mktemp)"
  xcrun notarytool submit "$file" "${args[@]}" --wait --output-format json > "$out"
  status="$(plutil -extract status raw -o - "$out" 2>/dev/null || echo unknown)"
  id="$(plutil -extract id raw -o - "$out" 2>/dev/null || true)"
  rm -f "$out"
  echo "   submission ${id:-?}: $status"
  if [[ "$status" != "Accepted" ]]; then
    [[ -n "$id" ]] && xcrun notarytool log "$id" "${args[@]}" || true
    echo "error: notarization of $(basename "$file") was not accepted ($status)" >&2
    return 1
  fi
}

echo "==> codesign ($SIGN_IDENTITY)"
# Secure timestamps need Apple's server and a real identity; skip for ad-hoc.
TIMESTAMP="--timestamp"
[[ "$SIGN_IDENTITY" == "-" ]] && TIMESTAMP="--timestamp=none"
codesign --force --options runtime "$TIMESTAMP" --sign "$SIGN_IDENTITY" \
  --identifier "$BUNDLE_ID" \
  --entitlements packaging/entitlements.plist "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
REQUIREMENT="$(codesign -d -r- "$APP" 2>&1 | sed -n 's/^designated => //p')"
echo "   designated requirement: $REQUIREMENT"
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  # A stable requirement is the whole point of a real identity: it must name
  # the bundle identifier and (when known) the team, never a code hash.
  if [[ "$REQUIREMENT" != *"identifier \"$BUNDLE_ID\""* || "$REQUIREMENT" == *cdhash* ]]; then
    echo "error: designated requirement is not stable: $REQUIREMENT" >&2
    exit 1
  fi
  if [[ -n "${APPLE_TEAM_ID:-}" && "$REQUIREMENT" != *"$APPLE_TEAM_ID"* ]]; then
    echo "error: designated requirement does not name team $APPLE_TEAM_ID" >&2
    exit 1
  fi
fi

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  ZIP="$DIST/Lumora-$VERSION.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  notarize "$ZIP"
  rm -f "$ZIP"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute -vv "$APP"
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
  echo "==> codesign $(basename "$DMG")"
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
  codesign --verify --strict --verbose=2 "$DMG"
  if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    notarize "$DMG"
    xcrun stapler staple "$DMG"
    xcrun stapler validate "$DMG"
    spctl --assess --type open --context context:primary-signature -vv "$DMG"
  fi
fi
echo "==> Done: $DMG"
