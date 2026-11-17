#!/bin/sh
# Snapreq installer for Linux and macOS. Installs for the current user only, no root needed.
#   curl -fsSL https://raw.githubusercontent.com/luxend0/snapreq/main/install.sh | sh
# Uninstall: sh install.sh --uninstall
set -eu

REPO=luxend0/snapreq
BIN="${HOME}/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN/snapreq" "$APPS/snapreq.desktop" "$ICONS/snapreq.svg"
    echo "Snapreq removed."
    exit 0
fi

os=$(uname -s)
arch=$(uname -m)
case "$os-$arch" in
    Linux-x86_64) asset=snapreq-linux-x86_64.tar.gz ;;
    Darwin-arm64) asset=snapreq-macos-arm64.tar.gz ;;
    *) echo "No prebuilt Snapreq for $os $arch. Build it with: cargo build --release" >&2; exit 1 ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $asset ..."
curl -fsSL "https://github.com/$REPO/releases/latest/download/$asset" -o "$tmp/snapreq.tar.gz"
tar -xzf "$tmp/snapreq.tar.gz" -C "$tmp"

mkdir -p "$BIN"
install -m 755 "$tmp/snapreq" "$BIN/snapreq"
if [ "$os" = Linux ]; then
    mkdir -p "$APPS" "$ICONS"
    # full path: desktop launchers often start without ~/.local/bin in PATH
    sed "s|^Exec=snapreq|Exec=$BIN/snapreq|" "$tmp/snapreq.desktop" > "$APPS/snapreq.desktop"
    install -m 644 "$tmp/snapreq.svg" "$ICONS/snapreq.svg"
fi

echo "Installed: $BIN/snapreq"
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "Note: $BIN is not in your PATH. Add it, or start Snapreq with the full path." ;;
esac
