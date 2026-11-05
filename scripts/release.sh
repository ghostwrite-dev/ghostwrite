#!/bin/sh
# Builds a signed, notarized Ghostwrite.zip that macOS will open without a warning.
#
#   DEVELOPER_ID="Developer ID Application: Your Name (TEAMID)" \
#   NOTARY_PROFILE=ghostwrite \
#   UPDATE_FEED=https://example.com/ghostwrite/latest.json \
#   DOWNLOAD_URL=https://example.com/ghostwrite/Ghostwrite.zip \
#   scripts/release.sh
#
# It needs an Apple Developer account. NOTARY_PROFILE is made once with
# `xcrun notarytool store-credentials`, which keeps it in the Keychain.
# With `--local`, it signs for this Mac only and skips notarizing, to try the bundle.
set -eu
cd "$(dirname "$0")/.."

LOCAL=false
[ "${1:-}" = "--local" ] && LOCAL=true
if ! $LOCAL; then
    : "${DEVELOPER_ID:?set DEVELOPER_ID, or use --local}"
    : "${NOTARY_PROFILE:?set NOTARY_PROFILE, or use --local}"
fi

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT=target/release-bundle
APP=$OUT/Ghostwrite.app
rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Built in, so the app knows where to look for a newer version.
GHOSTWRITE_UPDATE_FEED="${UPDATE_FEED:-}" cargo build --release
scripts/build-plugins.sh
cp target/release/ghostwrite "$APP/Contents/MacOS/ghostwrite"
cp -R target/bundled-plugins "$APP/Contents/Resources/plugins"
cp assets/icon/Ghostwrite.icns "$APP/Contents/Resources/Ghostwrite.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE"
scripts/info-plist.sh "$VERSION" > "$APP/Contents/Info.plist"

if $LOCAL; then
    codesign --force --sign - "$APP"
else
    # The local model's worker maps its model and compiles Metal shaders as it starts.
    codesign --force --options runtime --timestamp --entitlements scripts/entitlements.plist --sign "$DEVELOPER_ID" "$APP"
fi
codesign --verify --strict "$APP"

ZIP=$OUT/Ghostwrite-$VERSION.zip
ditto -c -k --keepParent "$APP" "$ZIP"

if ! $LOCAL; then
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP"
    rm "$ZIP"
    ditto -c -k --keepParent "$APP" "$ZIP"
    xcrun stapler validate "$APP"
fi

# What's published beside the zip, at UPDATE_FEED.
printf '{ "version": "%s", "url": "%s" }\n' "$VERSION" "${DOWNLOAD_URL:-}" > "$OUT/latest.json"
echo "$ZIP"
