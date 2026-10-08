#!/bin/sh
# Build and run the moment-of-use preview (Ink Sweep, orb fallback, Ghost Diff)
# against the production Swift sources. Arguments are passed through, e.g.
#   sh examples/run-native-moment-preview.sh --mode ghost-diff --appearance dark
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
native="$root/apps/selara/native"
out="$root/target/native-moment-preview"
mkdir -p "$out"
xcrun swiftc -swift-version 5 -O -module-name SelaraMomentPreview \
  "$native"/ThinkingOrbsKit/Sources/ThinkingOrbsKit/*.swift \
  "$native"/*.swift \
  "$root/examples/native-progress-preview/Moment/main.swift" \
  -o "$out/selara-moment-preview"
exec "$out/selara-moment-preview" "$@"
