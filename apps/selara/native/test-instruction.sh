#!/bin/sh
# Headless tests for the custom-instruction popover: chip compilation, recall
# stepping, placement math, event encoding, and the controller's key paths.
set -eu
native_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$native_dir/../../.." && pwd)
build_dir="$repo_root/target/native-instruction-tests"
mkdir -p "$build_dir"
xcrun swiftc -swift-version 5 -O -module-name SelaraInstructionTests \
  "$native_dir"/Instruction*.swift "$native_dir/Tests/Instruction/main.swift" \
  -o "$build_dir/native-instruction-tests"
exec "$build_dir/native-instruction-tests" "$@"
