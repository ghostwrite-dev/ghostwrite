#!/bin/sh
# Builds the plugins that come with Ghostwrite, for WebAssembly, into
# target/bundled-plugins: what the app loads, and the installers copy.
set -eu
cd "$(dirname "$0")/.."

rustup target add wasm32-wasip2 >/dev/null 2>&1 || true
rm -rf target/bundled-plugins
for plugin in typst pdf image; do
    cargo build --release --target wasm32-wasip2 --manifest-path "plugins/$plugin/Cargo.toml" --target-dir target/plugin-builds
    out="target/bundled-plugins/$plugin"
    mkdir -p "$out"
    cp "target/plugin-builds/wasm32-wasip2/release/ghostwrite_${plugin}_plugin.wasm" "$out/$plugin.wasm"
    cp "plugins/$plugin/plugin.json" "$out/"
done
