#!/bin/sh
# Builds Ghostwrite and installs it for this user, with a desktop entry.
# It needs Rust, cmake, a C compiler, and the development files for Vulkan, X11, Wayland and
# fontconfig: on Debian and Ubuntu,
#   apt install cmake clang libvulkan-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libssl-dev
set -eu
cd "$(dirname "$0")/.."

BIN="$HOME/.local/bin"
SHARE="$HOME/.local/share/ghostwrite"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

cargo build --release
scripts/build-plugins.sh
mkdir -p "$BIN" "$APPS" "$SHARE"
install -m 755 target/release/ghostwrite "$BIN/ghostwrite"
rm -rf "$SHARE/plugins"
cp -R target/bundled-plugins "$SHARE/plugins"
cat > "$APPS/ghostwrite.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Ghostwrite
Comment=A modal editor where a model's changes come as small steps you accept
Exec=$BIN/ghostwrite %F
Terminal=false
Categories=Development;TextEditor;
MimeType=text/plain;application/pdf;
DESKTOP
echo "Installed $BIN/ghostwrite"
