#!/usr/bin/env bash
# Builds the Linux release packages into dist/: a .tar.gz (binary, desktop entry, icons, install.sh),
# a .deb (cargo-deb) and an AppImage (appimagetool, downloaded when not on the PATH).
# Run from the repository root; the architecture is the machine's (x86_64 or aarch64).
set -euo pipefail
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
out=dist
mkdir -p "$out"

cargo build --profile dist --locked
bin=target/dist/branchblitz

# .tar.gz: unpack and run ./install.sh (to ~/.local), or run ./branchblitz directly.
name="branchblitz-$version-linux-$arch"
stage=$(mktemp -d)
mkdir -p "$stage/$name/scripts" "$stage/$name/assets"
cp "$bin" branchblitz.desktop README.md LICENSE-MIT LICENSE-APACHE "$stage/$name/"
cp -r assets/icons "$stage/$name/assets/"
cp scripts/install-desktop.sh "$stage/$name/scripts/"
cat > "$stage/$name/install.sh" <<'EOF'
#!/usr/bin/env bash
# Installs BranchBlitz for the current user: ~/.local/bin/branchblitz plus desktop entry and icon.
set -euo pipefail
cd "$(dirname "$0")"
install -Dm755 branchblitz "$HOME/.local/bin/branchblitz"
scripts/install-desktop.sh
echo "Installed ~/.local/bin/branchblitz (make sure ~/.local/bin is on your PATH)."
EOF
chmod +x "$stage/$name/install.sh"
tar -czf "$out/$name.tar.gz" -C "$stage" "$name"
rm -rf "$stage"

# .deb (installs to /usr, depends on git).
cargo deb --profile dist --no-build --no-strip --output "$out/"

# AppImage: one file, runs on most distributions (needs FUSE, like every AppImage).
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [ -z "$tool" ]; then
    tool=$(mktemp -d)/appimagetool
    curl -sfL -o "$tool" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
    chmod +x "$tool"
fi
appdir=$(mktemp -d)/BranchBlitz.AppDir
install -Dm755 "$bin" "$appdir/usr/bin/branchblitz"
install -Dm644 branchblitz.desktop "$appdir/branchblitz.desktop"
install -Dm644 assets/icons/branchblitz-256.png "$appdir/branchblitz.png"
install -Dm644 assets/icons/branchblitz-256.png "$appdir/usr/share/icons/hicolor/256x256/apps/branchblitz.png"
ln -s usr/bin/branchblitz "$appdir/AppRun"
# --appimage-extract-and-run: works without FUSE (CI containers).
ARCH=$arch "$tool" --appimage-extract-and-run --no-appstream "$appdir" "$out/branchblitz-$version-$arch.AppImage" >/dev/null
rm -rf "$(dirname "$appdir")"

ls -l "$out"
