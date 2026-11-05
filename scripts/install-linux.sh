#!/bin/sh
# Builds Ghostwrite and installs it for this user, with a desktop entry, and the
# default plugins, or none with --no-plugins.
# It needs Rust, cmake, a C compiler, and the development files for Vulkan, X11, Wayland and
# fontconfig: on Debian and Ubuntu,
#   apt install cmake clang libvulkan-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libssl-dev
set -eu
cd "$(dirname "$0")/.."

BIN="$HOME/.local/bin"
SHARE="$HOME/.local/share/ghostwrite"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

PLUGINS=yes
case "${1:-}" in
    --no-plugins) PLUGINS=no ;;
    "") ;;
    *) echo "usage: $0 [--no-plugins]" >&2; exit 2 ;;
esac

cargo build --release
if [ "$PLUGINS" = yes ]; then scripts/build-plugins.sh; fi
mkdir -p "$BIN" "$APPS" "$SHARE"
install -m 755 target/release/ghostwrite "$BIN/ghostwrite"
rm -rf "$SHARE/plugins"
if [ "$PLUGINS" = yes ]; then cp -R target/bundled-plugins "$SHARE/plugins"; fi
install -m 644 LICENSE "$SHARE/LICENSE"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"
mkdir -p "$ICONS"
install -m 644 assets/icon/ghostwrite.png "$ICONS/ghostwrite.png"
cat > "$APPS/ghostwrite.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Ghostwrite
Comment=A modal editor where a model's changes come as small steps you accept
Exec=$BIN/ghostwrite %F
Icon=ghostwrite
Terminal=false
Categories=Development;TextEditor;
MimeType=text/plain;application/pdf;
DESKTOP
echo "Installed $BIN/ghostwrite"
