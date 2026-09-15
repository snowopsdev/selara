#!/bin/sh
set -eu
native_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$native_dir/../../.." && pwd)
build_dir="$repo_root/target/native-progress-tests"
mkdir -p "$build_dir"
xcrun swiftc -swift-version 5 -O -module-name SelaraProgressTests \
  "$native_dir"/ThinkingOrbsKit/Sources/ThinkingOrbsKit/*.swift \
  "$native_dir/Progress.swift" "$native_dir/Tests/main.swift" \
  -o "$build_dir/native-progress-tests"
exec "$build_dir/native-progress-tests" "$@"
