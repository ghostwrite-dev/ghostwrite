#!/bin/sh
# Assembles a Ghostwrite plugin into a folder the editor loads: its code if it has any
# (a crate, built for WebAssembly), the grammars its grammars.txt lists (downloaded once
# and checked against their sha256), and the rest of its files as they are. It's what
# .github/workflows/plugin.yml builds a plugin's repository with, and what
# scripts/build-plugins.sh builds a default-plugins checkout with.
#
#     plugin-build.sh [plugin folder] [out folder]
#
# The plugin folder is the current one unless given, and the out folder is
# target/plugin/<name> in it. CARGO_TARGET_DIR says where its code is built
# (target in the plugin folder by default), and GRAMMARS_DIR where grammars are kept.
set -eu

source=$(cd "${1:-.}" && pwd)
[ -f "$source/plugin.json" ] || { echo "$source has no plugin.json" >&2; exit 1; }

# A plugin's name is the first "name" in its plugin.json, which comes before anything else.
name=$(sed -n 's/^ *"name": *"\([^"]*\)".*/\1/p' "$source/plugin.json" | head -n 1)
[ -n "$name" ] || { echo "$source/plugin.json doesn't say its name" >&2; exit 1; }
out=${2:-$source/target/plugin/$name}
grammars=${GRAMMARS_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/ghostwrite/grammars}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

rm -rf "$out"
mkdir -p "$out"

if [ -f "$source/Cargo.toml" ]; then
    rustup target add wasm32-wasip2 >/dev/null 2>&1 || true
    target=${CARGO_TARGET_DIR:-$source/target}
    locked=
    [ -f "$source/Cargo.lock" ] && locked=--locked
    cargo build --release $locked --target wasm32-wasip2 --manifest-path "$source/Cargo.toml" --target-dir "$target"
    crate=$(sed -n 's/^name *= *"\([^"]*\)".*/\1/p' "$source/Cargo.toml" | head -n 1 | tr - _)
    code=$(sed -n 's/^ *"code": *"\([^"]*\)".*/\1/p' "$source/plugin.json" | head -n 1)
    cp "$target/wasm32-wasip2/release/$crate.wasm" "$out/${code:-$name.wasm}"
fi

if [ -f "$source/grammars.txt" ]; then
    mkdir -p "$grammars"
    grep -v '^#' "$source/grammars.txt" | while read -r sum url; do
        [ -n "$url" ] || continue
        file="$grammars/$(basename "$url")"
        if [ ! -f "$file" ] || [ "$(sha256 "$file")" != "$sum" ]; then
            curl -fsSL -o "$file.part" "$url"
            if [ "$(sha256 "$file.part")" != "$sum" ]; then
                echo "$url isn't the grammar grammars.txt says" >&2
                rm -f "$file.part"
                exit 1
            fi
            mv "$file.part" "$file"
        fi
        cp "$file" "$out/"
    done
fi

# The rest, as it is: not what builds it, nor what's there for the repository.
for file in "$source"/*; do
    case $(basename "$file") in
    Cargo.toml | Cargo.lock | src | grammars.txt | target | README* | LICENSE*) ;;
    *) cp -R "$file" "$out/" ;;
    esac
done

echo "$out"
