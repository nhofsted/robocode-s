#!/bin/sh
# Adds robocode-s to the application menu of the current user: a .desktop file, the
# icon and the recordings' file types in ~/.local/share, pointing at the programs next
# to this folder.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm644 "$root/share/robocode-s.png" "$data/icons/hicolor/256x256/apps/robocode-s.png"
mkdir -p "$data/applications"
sed -e "s|^Exec=.*|Exec=$root/robocode-s-gui %f|" \
    -e "s|^Icon=.*|Icon=$data/icons/hicolor/256x256/apps/robocode-s.png|" \
    "$root/share/robocode-s.desktop" > "$data/applications/robocode-s.desktop"
if [ -f "$root/share/robocode-s-mime.xml" ]; then
  install -Dm644 "$root/share/robocode-s-mime.xml" "$data/mime/packages/robocode-s.xml"
  if command -v update-mime-database >/dev/null 2>&1; then
    update-mime-database "$data/mime" >/dev/null 2>&1 || true
  fi
fi
echo "installed $data/applications/robocode-s.desktop"
