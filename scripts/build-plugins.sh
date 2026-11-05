#!/bin/sh
# Builds the plugins that come with Ghostwrite into target/bundled-plugins: what the
# app loads, and the installers copy. Each is a folder in plugins/, made of its code
# if it has any (a crate, built for WebAssembly), the grammars its grammars.txt lists
# (downloaded once, into target/grammars, and checked against their sha256), and the
# rest of its files as they are.
set -eu
cd "$(dirname "$0")/.."

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

rustup target add wasm32-wasip2 >/dev/null 2>&1 || true
rm -rf target/bundled-plugins
mkdir -p target/grammars
for source in plugins/*/; do
    plugin=$(basename "$source")
    out="target/bundled-plugins/$plugin"
    mkdir -p "$out"
    if [ -f "$source/Cargo.toml" ]; then
        cargo build --release --target wasm32-wasip2 --manifest-path "$source/Cargo.toml" --target-dir target/plugin-builds
        cp "target/plugin-builds/wasm32-wasip2/release/ghostwrite_${plugin}_plugin.wasm" "$out/$plugin.wasm"
    fi
    if [ -f "$source/grammars.txt" ]; then
        grep -v '^#' "$source/grammars.txt" | while read -r sum url; do
            [ -n "$url" ] || continue
            file="target/grammars/$(basename "$url")"
            if [ ! -f "$file" ] || [ "$(sha256 "$file")" != "$sum" ]; then
                curl -fsSL -o "$file.part" "$url"
                if [ "$(sha256 "$file.part")" != "$sum" ]; then
                    echo "$url isn't the grammar $source/grammars.txt says" >&2
                    rm -f "$file.part"
                    exit 1
                fi
                mv "$file.part" "$file"
            fi
            cp "$file" "$out/"
        done
    fi
    for file in "$source"*; do
        case $(basename "$file") in
        Cargo.toml | Cargo.lock | src | grammars.txt) ;;
        *) cp -R "$file" "$out/" ;;
        esac
    done
done
