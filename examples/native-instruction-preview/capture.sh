#!/bin/sh
# Screenshot the instruction popover preview in each appearance and state.
# Needs Screen Recording permission for the terminal running it.
#   sh examples/native-instruction-preview/capture.sh [/tmp/selara-native-instruction]
set -eu
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=${1:-/tmp/selara-native-instruction}
mkdir -p "$out"
mkdir -p "$repo_root/target/native-instruction-preview"
bin="$repo_root/target/native-instruction-preview/selara-instruction-preview"
xcrun swiftc -swift-version 5 -O -module-name SelaraInstructionPreview \
  "$repo_root"/apps/selara/native/Instruction*.swift \
  "$repo_root/examples/native-instruction-preview/main.swift" -o "$bin"
for appearance in light dark; do
  for state in off chips detail recall saved above; do
    info="$out/.info"
    rm -f "$info"
    flag=""
    [ "$appearance" = dark ] && flag="--dark"
    SELARA_PREVIEW_INFO="$info" "$bin" $flag --state "$state" >/dev/null 2>&1 &
    pid=$!
    tries=0
    while [ ! -s "$info" ] && [ $tries -lt 50 ]; do sleep 0.1; tries=$((tries + 1)); done
    sleep 0.4
    read -r panel _doc region <"$info" || true
    screencapture -x -o -l "$panel" "$out/popover-$appearance-$state.png"
    screencapture -x -R "$region" "$out/context-$appearance-$state.png"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
done
rm -f "$out/.info"
ls "$out"
