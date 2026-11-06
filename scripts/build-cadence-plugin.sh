#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --offline --locked --manifest-path platform-plugin/Cargo.toml
mkdir -p dist
cp platform-plugin/target/release/librebit_platform_plugin.dylib dist/rebit-rust.dylib
printf 'Built %s\n' "$PWD/dist/rebit-rust.dylib"
printf 'Open it with Cadence > Open... and select rebit-rust.dylib.\n'
