#!/bin/sh
set -eu
preview_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec swift run --package-path "$preview_root/examples/native-progress-preview" \
  --scratch-path "$preview_root/target/progress-preview/swift-build" \
  -c release selara-progress-preview
