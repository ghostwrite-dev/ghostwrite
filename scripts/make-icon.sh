#!/bin/sh
# Renders assets/icon/ghostwrite.svg into the icons the installers use, next to it:
# Ghostwrite.icns (macOS, needs iconutil), ghostwrite.png (Linux), ghostwrite.ico (Windows).
set -e
cd "$(dirname "$0")/.."
OUT=assets/icon
cargo run --quiet --release --manifest-path scripts/icon-render/Cargo.toml --target-dir target/icon-render -- "$OUT/ghostwrite.svg" "$OUT"
if command -v iconutil >/dev/null; then
    iconutil -c icns "$OUT/ghostwrite.iconset" -o "$OUT/Ghostwrite.icns"
fi
rm -rf "$OUT/ghostwrite.iconset"
ls "$OUT"
