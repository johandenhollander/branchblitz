#!/usr/bin/env bash
# Installs the desktop entry and the app icon for the current user, so launchers and docks show
# BranchBlitz with its icon. Run after `cargo install --path .`.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
for png in "$root"/assets/icons/branchblitz-*.png; do
    size=${png##*-}
    size=${size%.png}
    install -Dm644 "$png" "$data/icons/hicolor/${size}x${size}/apps/branchblitz.png"
done
install -Dm644 "$root/assets/icons/branchblitz.svg" "$data/icons/hicolor/scalable/apps/branchblitz.svg"
install -Dm644 "$root/branchblitz.desktop" "$data/applications/branchblitz.desktop"
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$data/applications" 2>/dev/null || true
