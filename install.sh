#!/bin/sh
# Builds Ghostwrite and installs it as /Applications/Ghostwrite.app.
set -eu
cd "$(dirname "$0")"

APP=/Applications/Ghostwrite.app
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

cargo build --release

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

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Ghostwrite</string>
    <key>CFBundleDisplayName</key><string>Ghostwrite</string>
    <key>CFBundleIdentifier</key><string>app.ghostwrite.Ghostwrite</string>
    <key>CFBundleExecutable</key><string>ghostwrite</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Text</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.text</string>
                <string>net.daringfireball.markdown</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
EOF

codesign --force --sign - "$APP"
"$LSREGISTER" -f "$APP"
mdimport "$APP"
echo "Installed $APP ($VERSION)"
