#!/usr/bin/env bash
set -euo pipefail
api="$(realpath "${1:?Pass the API executable to test}")"
chmod +x "$api"
mkdir -p .tools/engine
archive=.tools/engine.tar.gz
checksum=9cb7d3256716195852a8aa89021ff3ff36cb7c03c0720f2b8a8a5b449858b0ec
if ! printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --status 2>/dev/null; then
  curl --fail --location --silent --show-error https://github.com/securecodelabs/wifi-gpu-public/releases/download/v0.17.2/wifi-gpu-v0.17.2-linux-amd64.tar.gz --output "$archive"
fi
printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check
tar -xzf "$archive" -C .tools/engine
env -u WIFI_TEST_HIP WIFI_TEST_ENGINE="$PWD/.tools/engine/bin/wifi-gpu" WIFI_TEST_API="$api" \
  cargo +1.93.0 test --locked --test engine_integration -- --ignored --test-threads=1
