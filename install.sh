#!/usr/bin/env bash
set -euo pipefail

APP_NAME="appdrop"
APP_DIR="$HOME/.local/opt/appdrop"
BIN="$APP_DIR/appdrop"
APP_DIRS="$HOME/.local/share/applications"
MIME_DIR="$HOME/.local/share/mime/packages"

SOURCE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/target/release/appdrop"

if [[ ! -f "$SOURCE" ]]; then
  echo "Release binary not found."
  echo "Build it first: cargo build --release"
  exit 1
fi

for command in update-mime-database xdg-mime; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing command: $command"
    echo "On Arch Linux, install shared-mime-info and xdg-utils."
    exit 1
  fi
done

mkdir -p "$APP_DIR" "$APP_DIRS" "$MIME_DIR"

# Install AppDrop itself.
install -Dm755 "$SOURCE" "$BIN"

# Register the AppImage MIME type for both common extensions.
cat >"$MIME_DIR/appdrop-appimage.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/vnd.appimage">
    <comment>AppImage application</comment>
    <glob pattern="*.AppImage"/>
    <glob pattern="*.appimage"/>
  </mime-type>
</mime-info>
XML

update-mime-database "$HOME/.local/share/mime"

# Escape characters that have special meaning in desktop Exec fields.
ESCAPED_BIN="${BIN//\\/\\\\}"
ESCAPED_BIN="${ESCAPED_BIN//\"/\\\"}"

# Register AppDrop as a desktop application and file handler.
cat >"$APP_DIRS/appdrop.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=AppDrop
Comment=Install and manage AppImage applications
Exec="$ESCAPED_BIN" %f
Icon=application-x-executable
Terminal=false
Categories=Utility;
MimeType=application/vnd.appimage;
StartupNotify=true
EOF

chmod 644 "$APP_DIRS/appdrop.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIRS" || true
fi

# Make AppDrop the default handler for AppImage files.
xdg-mime default appdrop.desktop application/vnd.appimage

echo
echo "AppDrop was installed successfully!"
echo "Application: $BIN"
echo "Desktop entry: $APP_DIRS/appdrop.desktop"
echo
echo "Try opening an AppImage from your file manager."
