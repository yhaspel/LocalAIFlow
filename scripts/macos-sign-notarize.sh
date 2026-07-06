#!/usr/bin/env bash
# Build, sign, notarize and staple Local AI Flow for macOS distribution
# (Developer ID — NOT the App Store sandbox, which is incompatible with
# system-wide AX insertion and synthetic events).
#
# All secrets come from the environment — never hardcode them:
#   APPLE_SIGNING_IDENTITY   e.g. "Developer ID Application: Jane Doe (TEAMID123)"
#   APPLE_TEAM_ID            e.g. "TEAMID123"
#   APPLE_ID                 Apple ID email used for notarization
#   APPLE_APP_PASSWORD       app-specific password for notarytool
#
# Usage: scripts/macos-sign-notarize.sh [--skip-build]
set -euo pipefail

: "${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY}"
: "${APPLE_TEAM_ID:?set APPLE_TEAM_ID}"
: "${APPLE_ID:?set APPLE_ID}"
: "${APPLE_APP_PASSWORD:?set APPLE_APP_PASSWORD}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENTITLEMENTS="$ROOT/scripts/macos-entitlements.plist"
APP_NAME="Local AI Flow.app"

cd "$ROOT"

if [[ "${1:-}" != "--skip-build" ]]; then
  # Ensure the .icns exists (generated from icon.png).
  "$ROOT/scripts/make-icns.sh"
  # tauri-cli: `cargo install tauri-cli --version ^2` if missing.
  cargo tauri build --bundles app,dmg
fi

APP_PATH="$(find "$ROOT" -path "*/release/bundle/macos/$APP_NAME" | head -1)"
[[ -d "$APP_PATH" ]] || { echo "app bundle not found — build failed?"; exit 1; }

echo "==> codesign (Hardened Runtime) — $APP_PATH"
# Sign any bundled helpers first (inside-out signing).
find "$APP_PATH/Contents/MacOS" -type f ! -name "local-ai-flow" -perm +111 2>/dev/null | while read -r helper; do
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$APPLE_SIGNING_IDENTITY" "$helper"
done
codesign --force --deep --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" \
  --sign "$APPLE_SIGNING_IDENTITY" "$APP_PATH"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "==> notarize"
ZIP="$(mktemp -d)/LocalAIFlow.zip"
ditto -c -k --keepParent "$APP_PATH" "$ZIP"
xcrun notarytool submit "$ZIP" \
  --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_APP_PASSWORD" \
  --wait

echo "==> staple"
xcrun stapler staple "$APP_PATH"
xcrun stapler validate "$APP_PATH"

DMG_PATH="$(find "$ROOT" -path "*/release/bundle/dmg/*.dmg" | head -1 || true)"
if [[ -n "${DMG_PATH:-}" ]]; then
  echo "==> sign + notarize dmg"
  codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG_PATH"
  xcrun notarytool submit "$DMG_PATH" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$DMG_PATH"
  echo "done: $DMG_PATH"
fi
echo "done: $APP_PATH"
