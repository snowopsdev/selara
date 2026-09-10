#!/usr/bin/env bash
# Render the Homebrew cask (app DMG) and formula (CLI tarball) for a release.
#
#   scripts/release/render-homebrew.sh <version> <dmg sha256> <cli tarball sha256> <out dir> [signed|unsigned]
#
# Writes <out dir>/Casks/selara.rb and <out dir>/Formula/selara.rb.
set -euo pipefail

VERSION=${1:?version}
DMG_SHA=${2:?dmg sha256}
CLI_SHA=${3:?cli sha256}
OUT=${4:?out dir}
SIGNED=${5:-unsigned}

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$OUT/Casks" "$OUT/Formula"

if [ "$SIGNED" = "signed" ]; then
  CAVEAT=""
else
  CAVEAT="This build is not notarized by Apple yet. If macOS reports the app as damaged, run: xattr -d com.apple.quarantine /Applications/Selara.app"
fi

render() {
  sed -e "s|@VERSION@|$VERSION|g" \
      -e "s|@DMG_SHA256@|$DMG_SHA|g" \
      -e "s|@CLI_SHA256@|$CLI_SHA|g" \
      -e "s|@UNSIGNED_CAVEAT@|$CAVEAT|g" "$1"
}

render "$ROOT/homebrew/Casks/selara.rb.tmpl" > "$OUT/Casks/selara.rb"
render "$ROOT/homebrew/Formula/selara.rb.tmpl" > "$OUT/Formula/selara.rb"
echo "rendered $OUT/Casks/selara.rb and $OUT/Formula/selara.rb"
