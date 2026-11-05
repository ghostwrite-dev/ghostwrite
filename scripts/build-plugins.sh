#!/bin/sh
# Puts the plugins that come with Ghostwrite in target/bundled-plugins: what the app
# loads, and the installers copy. They're released from ghostwrite-dev/default-plugins
# and pinned in plugins.lock, a line each: `<name> <tag> <url> <sha256>`. Each zip is
# downloaded once, into target/plugin-downloads, checked against its sha256, and
# unzipped. The plugins only the tests use, pinned the same way in test-plugins.lock,
# go to target/test-plugins, which nothing ships.
#
# To work on them: GHOSTWRITE_PLUGINS_SRC=<a default-plugins checkout> builds those in
# plugins.lock from it instead, with ghostwrite-dev/plugin-build's build.sh (where
# PLUGIN_BUILD says, or plugin-build/build.sh beside the checkout).
set -eu
cd "$(dirname "$0")/.."

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

# The plugins `lock` pins, downloaded, checked and unzipped into `out`.
fetch() {
    lock=$1 out=$2
    rm -rf "$out"
    mkdir -p "$out" target/plugin-downloads
    grep -v '^#' "$lock" | while read -r name tag url sum; do
        [ -n "$sum" ] || continue
        zip="target/plugin-downloads/$name-$tag.zip"
        if [ ! -f "$zip" ] || [ "$(sha256 "$zip")" != "$sum" ]; then
            curl -fsSL -o "$zip.part" "$url"
            if [ "$(sha256 "$zip.part")" != "$sum" ]; then
                echo "$url isn't the $name $lock pins" >&2
                rm -f "$zip.part"
                exit 1
            fi
            mv "$zip.part" "$zip"
        fi
        # Each zip is one folder, named for its plugin.
        unzip -q -o "$zip" -d "$out"
        [ -f "$out/$name/plugin.json" ] || { echo "$zip has no $name/plugin.json" >&2; exit 1; }
    done
}

if [ -n "${GHOSTWRITE_PLUGINS_SRC:-}" ]; then
    src=$(cd "$GHOSTWRITE_PLUGINS_SRC" && pwd)
    build=${PLUGIN_BUILD:-$src/../plugin-build/build.sh}
    [ -f "$build" ] || { echo "no plugin-build at $build: set PLUGIN_BUILD" >&2; exit 1; }
    rm -rf target/bundled-plugins
    grep -v '^#' plugins.lock | while read -r name _; do
        [ -n "$name" ] || continue
        CARGO_TARGET_DIR="$PWD/target/plugin-builds" GRAMMARS_DIR="$PWD/target/grammars" \
            sh "$build" "$src/$name" "target/bundled-plugins/$name" >/dev/null
    done
else
    fetch plugins.lock target/bundled-plugins
fi
fetch test-plugins.lock target/test-plugins
