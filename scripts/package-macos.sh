#!/usr/bin/env bash
# Builds dist/BranchBlitz-<version>-macos-<arch>.zip: BranchBlitz.app for one target
# (aarch64-apple-darwin or x86_64-apple-darwin). Run on macOS from the repository root. Unsigned:
# the first start needs right-click, Open (or `xattr -dr com.apple.quarantine BranchBlitz.app`).
set -euo pipefail
target=${1:?usage: package-macos.sh <target triple>}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=${target%%-*}
out=dist
mkdir -p "$out"

cargo build --profile dist --locked --target "$target"

stage=$(mktemp -d)
app="$stage/BranchBlitz.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$target/dist/branchblitz" "$app/Contents/MacOS/branchblitz"

# The icon: an .icns from the PNG sizes.
iconset="$stage/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    cp "assets/icons/branchblitz-$size.png" "$iconset/icon_${size}x${size}.png"
done
for size in 16 32 128 256; do
    cp "assets/icons/branchblitz-$((size * 2)).png" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>BranchBlitz</string>
    <key>CFBundleDisplayName</key><string>BranchBlitz</string>
    <key>CFBundleIdentifier</key><string>io.github.johandenhollander.branchblitz</string>
    <key>CFBundleExecutable</key><string>branchblitz</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

ditto -c -k --keepParent "$app" "$out/BranchBlitz-$version-macos-$arch.zip"
rm -rf "$stage"
ls -l "$out"
