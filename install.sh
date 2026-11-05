#!/bin/sh
# Builds Ghostwrite and installs it as /Applications/Ghostwrite.app, with the
# default plugins, or none with --no-plugins.
set -eu
cd "$(dirname "$0")"

APP=/Applications/Ghostwrite.app
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

PLUGINS=yes
case "${1:-}" in
    --no-plugins) PLUGINS=no ;;
    "") ;;
    *) echo "usage: $0 [--no-plugins]" >&2; exit 2 ;;
esac

cargo build --release
if [ "$PLUGINS" = yes ]; then scripts/build-plugins.sh; fi

# Replace only our own bundle, never something else that happens to be there.
if [ -e "$APP" ]; then
    if [ "$(defaults read "$APP/Contents/Info" CFBundleIdentifier 2>/dev/null)" != "app.ghostwrite.Ghostwrite" ]; then
        echo "$APP exists and isn't Ghostwrite; leaving it alone." >&2
        exit 1
    fi
    rm -rf "$APP"
fi
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/ghostwrite "$APP/Contents/MacOS/ghostwrite"
if [ "$PLUGINS" = yes ]; then cp -R target/bundled-plugins "$APP/Contents/Resources/plugins"; fi
cp assets/icon/Ghostwrite.icns "$APP/Contents/Resources/Ghostwrite.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE"

scripts/info-plist.sh "$VERSION" > "$APP/Contents/Info.plist"

codesign --force --sign - "$APP"
"$LSREGISTER" -f "$APP"
mdimport "$APP"
echo "Installed $APP ($VERSION)"
