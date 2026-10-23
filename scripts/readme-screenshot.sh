#!/usr/bin/env bash
# Renders docs/screenshot.png for the README: the demo repository in dark (top left) and light
# (bottom right), split diagonally. Needs ImageMagick (magick). Run from the repository root.
set -euo pipefail
dir=/tmp/bb-readme
scripts/demo-repo.sh "$dir"
# Only render_screens: the merge render test changes the demo repo while it runs.
BB_DEMO="$dir/work" cargo test render_screens -- --ignored --quiet
read -r w h < <(magick identify -format "%w %h\n" /tmp/bb-dark.png)
magick -size "${w}x${h}" xc:black -fill white -draw "polygon $w,0 $w,$h 0,$h" "$dir/mask.png"
mkdir -p docs
magick /tmp/bb-dark.png /tmp/bb-light.png "$dir/mask.png" -composite docs/screenshot.png
echo "wrote docs/screenshot.png"
