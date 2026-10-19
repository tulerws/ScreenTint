#!/bin/sh
# Installs ScreenTint for the current user under ~/.local (use PREFIX=/usr/local with sudo for system-wide).
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
install -Dm755 "$here/screentint" "$prefix/bin/screentint"
install -Dm644 "$here/screentint.desktop" "$prefix/share/applications/screentint.desktop"
install -Dm644 "$here/screentint.png" "$prefix/share/icons/hicolor/512x512/apps/screentint.png"
echo "installed to $prefix"
