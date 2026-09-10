#!/usr/bin/env bash
# Verify packaging without launching the GUI or requiring Accessibility access.
set -euo pipefail
ROOT=${1:?repository directory}
APP="$ROOT/target/release/bundle/macos/Selara.app"
codesign --verify --deep --strict "$APP"
codesign --verify --strict "$APP/Contents/MacOS/selara"
"$APP/Contents/MacOS/selara" --help
shopt -s nullglob
DMGS=("$ROOT"/target/release/bundle/dmg/*.dmg)
if [ "${#DMGS[@]}" -ne 1 ]; then
  echo "Expected exactly one packaged DMG" >&2
  exit 1
fi
hdiutil verify "${DMGS[0]}"
