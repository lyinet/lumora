#!/usr/bin/env bash
# Build the release DMG with Developer ID signing + notarization when the
# signing secrets are present, and fall back to today's ad-hoc build when
# they are not. Used by CI; works locally with the same variables.
#
# Developer ID signing (all three required):
#   MACOS_CERT_P12        base64 of the exported "Developer ID Application"
#                         certificate + private key (.p12)
#   MACOS_CERT_PASSWORD   the .p12 export password
#   APPLE_TEAM_ID         10-character team ID, e.g. AB12CD34EF
#
# Notarization (either set; the API key wins when both are complete):
#   APPLE_ID + APPLE_APP_PASSWORD                Apple ID + app-specific password
#   NOTARY_KEY_ID + NOTARY_ISSUER_ID + NOTARY_KEY_P8
#                                                App Store Connect API key
#                                                (NOTARY_KEY_P8 = the .p8 file's
#                                                text, or base64 of it)
#
# Optional:
#   REQUIRE_NOTARIZATION=1   fail instead of shipping a signed but
#                            un-notarized build when notary creds are missing
#
# Nothing here prints a secret. The certificate lives in a temporary keychain
# that is deleted (and the keychain search list restored) on exit.
# See docs/signing.md.
set -euo pipefail

cd "$(dirname "$0")/.."
BUNDLE_SCRIPT="${LUMORA_BUNDLE_SCRIPT:-scripts/bundle.sh}"
IN_CI="${GITHUB_ACTIONS:-false}"

notice() { if [[ "$IN_CI" == "true" ]]; then echo "::notice title=Signing::$*"; else echo "notice: $*"; fi; }
warn() { if [[ "$IN_CI" == "true" ]]; then echo "::warning title=Signing::$*"; else echo "warning: $*" >&2; fi; }
fail() { if [[ "$IN_CI" == "true" ]]; then echo "::error title=Signing::$*"; else echo "error: $*" >&2; fi; exit 1; }
summary() { if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then echo "$*" >> "$GITHUB_STEP_SUMMARY"; fi; }
present() { [[ -n "${!1:-}" ]]; }

missing=()
for v in MACOS_CERT_P12 MACOS_CERT_PASSWORD APPLE_TEAM_ID; do
  present "$v" || missing+=("$v")
done

if (( ${#missing[@]} > 0 )); then
  # CI passes only SIGNING_SECRETS_PARTIAL to this path, never the secrets.
  if (( ${#missing[@]} < 3 )) || [[ "${SIGNING_SECRETS_PARTIAL:-false}" == "true" ]]; then
    warn "Some of MACOS_CERT_P12, MACOS_CERT_PASSWORD and APPLE_TEAM_ID are set but not all three; building ad-hoc instead."
  fi
  notice "No Developer ID certificate configured: building an ad-hoc signed DMG. macOS will ask for Screen Recording again after every update. See docs/signing.md to enable signed + notarized builds."
  summary "**Signing:** ad-hoc (no Developer ID secrets). Not notarized."
  unset MACOS_CERT_P12 MACOS_CERT_PASSWORD APPLE_APP_PASSWORD NOTARY_KEY_P8
  SIGN_IDENTITY="-" NOTARY_PROFILE="" exec "$BUNDLE_SCRIPT"
fi

[[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || fail "APPLE_TEAM_ID must be the 10-character team ID (letters and digits)."

notary_mode=""
if present NOTARY_KEY_ID && present NOTARY_ISSUER_ID && present NOTARY_KEY_P8; then
  notary_mode="api-key"
elif present APPLE_ID && present APPLE_APP_PASSWORD; then
  notary_mode="apple-id"
elif [[ "${REQUIRE_NOTARIZATION:-0}" == "1" ]]; then
  fail "REQUIRE_NOTARIZATION=1 but no complete notary credentials (APPLE_ID + APPLE_APP_PASSWORD, or NOTARY_KEY_ID + NOTARY_ISSUER_ID + NOTARY_KEY_P8)."
else
  warn "Developer ID certificate found but no complete notary credentials: the build is signed (stable identity) but NOT notarized, so Gatekeeper will block its first open."
fi

# Everything secret goes into this directory; the keychain too.
umask 077
WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/lumora-signing.XXXXXX")"
KEYCHAIN="$WORK/lumora-signing.keychain-db"
KEYCHAIN_PASSWORD="$(openssl rand -hex 24)"
if [[ "$IN_CI" == "true" ]]; then echo "::add-mask::$KEYCHAIN_PASSWORD"; fi
ORIGINAL_KEYCHAINS=()
while IFS= read -r line; do
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line%\"}"
  line="${line#\"}"
  [[ -n "$line" ]] && ORIGINAL_KEYCHAINS+=("$line")
done < <(security list-keychains -d user)

cleanup() {
  local rc=$?
  if (( ${#ORIGINAL_KEYCHAINS[@]} > 0 )); then
    security list-keychains -d user -s "${ORIGINAL_KEYCHAINS[@]}" >/dev/null 2>&1 || true
  fi
  security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

echo "==> Temporary keychain"
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-keychain-settings -lut 21600 "$KEYCHAIN"   # cargo builds take a while
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security list-keychains -d user -s "$KEYCHAIN" "${ORIGINAL_KEYCHAINS[@]}"

echo "==> Importing the Developer ID certificate"
P12="$WORK/cert.p12"
printf '%s' "$MACOS_CERT_P12" | tr -d '[:space:]' | base64 --decode > "$P12" \
  || fail "MACOS_CERT_P12 is not valid base64 (export with: base64 -i cert.p12 | pbcopy)."
security import "$P12" -k "$KEYCHAIN" -f pkcs12 -P "$MACOS_CERT_PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/security >/dev/null \
  || fail "Could not import MACOS_CERT_P12 (wrong MACOS_CERT_PASSWORD, or not a .p12 with its private key)."
rm -f "$P12"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null

IDENTITY_LINE="$(security find-identity -v -p codesigning "$KEYCHAIN" \
  | grep -F "\"Developer ID Application:" | grep -F "($APPLE_TEAM_ID)\"" | head -1 || true)"
[[ -n "$IDENTITY_LINE" ]] || fail "The certificate is not a valid \"Developer ID Application\" identity for team $APPLE_TEAM_ID (Apple Development / Distribution certificates cannot be notarized)."
IDENTITY_HASH="$(awk '{print $2}' <<<"$IDENTITY_LINE")"
IDENTITY_NAME="$(sed -n 's/.*"\(.*\)".*/\1/p' <<<"$IDENTITY_LINE")"
echo "   identity: $IDENTITY_NAME"

NOTARY_PROFILE=""
if [[ -n "$notary_mode" ]]; then
  echo "==> Storing notary credentials ($notary_mode) in the temporary keychain"
  NOTARY_PROFILE="lumora-ci"
  if [[ "$notary_mode" == "api-key" ]]; then
    KEY_FILE="$WORK/AuthKey_$NOTARY_KEY_ID.p8"
    if [[ "$NOTARY_KEY_P8" == *"BEGIN PRIVATE KEY"* ]]; then
      printf '%s\n' "$NOTARY_KEY_P8" > "$KEY_FILE"
    else
      printf '%s' "$NOTARY_KEY_P8" | tr -d '[:space:]' | base64 --decode > "$KEY_FILE" \
        || fail "NOTARY_KEY_P8 is neither the .p8 text nor base64 of it."
    fi
    xcrun notarytool store-credentials "$NOTARY_PROFILE" --keychain "$KEYCHAIN" \
      --key "$KEY_FILE" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" >/dev/null \
      || fail "notarytool rejected the App Store Connect API key (check NOTARY_KEY_ID / NOTARY_ISSUER_ID / NOTARY_KEY_P8; the key needs the Developer role)."
    rm -f "$KEY_FILE"
  else
    xcrun notarytool store-credentials "$NOTARY_PROFILE" --keychain "$KEYCHAIN" \
      --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" >/dev/null \
      || fail "notarytool rejected APPLE_ID / APPLE_APP_PASSWORD (use an app-specific password from appleid.apple.com, not the account password)."
  fi
fi

# The build itself never needs the raw secrets.
unset MACOS_CERT_P12 MACOS_CERT_PASSWORD APPLE_APP_PASSWORD NOTARY_KEY_P8

echo "==> Building (Developer ID${NOTARY_PROFILE:+ + notarization})"
SIGN_IDENTITY="$IDENTITY_HASH" NOTARY_PROFILE="$NOTARY_PROFILE" NOTARY_KEYCHAIN="$KEYCHAIN" \
  APPLE_TEAM_ID="$APPLE_TEAM_ID" "$BUNDLE_SCRIPT"

echo "==> Final checks"
APP="dist/Lumora.app"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -dv --verbose=2 "$APP" 2>&1 | grep -E '^(Identifier|Authority|TeamIdentifier|Timestamp|Runtime Version|CodeDirectory)' || true
for dmg in dist/*.dmg; do
  codesign --verify --strict --verbose=2 "$dmg"
done
if [[ -n "$NOTARY_PROFILE" ]]; then
  spctl --assess --type execute -vv "$APP"
  summary "**Signing:** Developer ID ($IDENTITY_NAME), notarized and stapled (app + DMG)."
else
  summary "**Signing:** Developer ID ($IDENTITY_NAME). **Not notarized** (no notary credentials)."
fi
