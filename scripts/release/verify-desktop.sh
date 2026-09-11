#!/usr/bin/env bash
# Verify packaging without launching the GUI or requiring Accessibility access.
set -euo pipefail
ROOT=${1:?repository directory}
APP="$ROOT/target/release/bundle/macos/Selara.app"
codesign --verify --deep --strict "$APP"
codesign --verify --strict "$APP/Contents/MacOS/selara"
"$APP/Contents/MacOS/selara" --help
if [ -x "$APP/Contents/MacOS/selara-codex" ]; then
  codesign --verify --strict "$APP/Contents/MacOS/selara-codex"
  env -i HOME="$HOME" PATH=/usr/bin:/bin "$APP/Contents/MacOS/selara-codex" --version
  test -s "$APP/Contents/Resources/runtime-notices/selara-codex.LICENSE"
  test -s "$APP/Contents/Resources/runtime-notices/selara-codex.NOTICE"
  test -s "$APP/Contents/Resources/runtime-notices/selara-codex.provenance.json"
fi
shopt -s nullglob
DMGS=("$ROOT"/target/release/bundle/dmg/*.dmg)
if [ "${#DMGS[@]}" -ne 1 ]; then
  echo "Expected exactly one packaged DMG" >&2
  exit 1
fi
hdiutil verify "${DMGS[0]}"
if [ "${SELARA_BUILD_MODE:-local}" = release ] || { [ "${SELARA_BUILD_MODE:-local}" = recovery ] && [ -x "$APP/Contents/MacOS/selara-codex" ]; }; then
  xcrun stapler validate "$APP"
  xcrun stapler validate "${DMGS[0]}"
  spctl --assess --type execute --verbose=2 "$APP"
  for binary in "$APP" "$APP/Contents/MacOS/selara" "$APP/Contents/MacOS/selara-codex"; do
    signature=$(codesign -dv --verbose=4 "$binary" 2>&1)
    case "$signature" in *"Authority=Developer ID Application:"*) ;; *) echo "Missing Developer ID signature" >&2; exit 1 ;; esac
    case "$signature" in *"TeamIdentifier=${APPLE_TEAM_ID:?}"*) ;; *) echo "Wrong Apple signing team" >&2; exit 1 ;; esac
  done
fi
if [ "${SELARA_BUILD_MODE:-local}" = ci ]; then
  node "$ROOT/scripts/release/verify-updater-build.mjs" "$ROOT"
fi
