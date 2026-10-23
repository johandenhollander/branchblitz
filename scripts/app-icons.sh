#!/usr/bin/env bash
# Renders the app icon from the design system (assets/brand/app-icon.svg, and app-icon-small.svg
# for 16–32 px where the facets would blur) as PNGs in the freedesktop sizes, into assets/icons/.
# Needs rsvg-convert (librsvg).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
for size in 512 256 128 64 48 32 24 16; do
    src=app-icon.svg
    [ "$size" -le 32 ] && src=app-icon-small.svg
    rsvg-convert -w "$size" -h "$size" "$root/assets/brand/$src" -o "$root/assets/icons/branchblitz-$size.png"
done
cp "$root/assets/brand/app-icon.svg" "$root/assets/icons/branchblitz.svg"
