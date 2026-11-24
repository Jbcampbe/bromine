#!/usr/bin/env bash
# Wrap an already-built release binary in a macOS .app and a dmg.
#
# usage: scripts/bundle-macos.sh <target-triple> <version> <arch>
# example: scripts/bundle-macos.sh aarch64-apple-darwin 0.1.0 aarch64
#
# The binary has to exist first: this script never invokes cargo, so CI can
# build with whatever profile and target it likes and the local path and the
# release path stay one flow.
#
# Ad-hoc signing is the default so an unsigned local bundle still launches.
# Set SIGN_IDENTITY to a Developer ID Application identity for a build that can
# be notarized; that path also turns on the hardened runtime and a secure
# timestamp, both of which notarization requires.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ $# -ne 3 ]]; then
    echo "usage: scripts/bundle-macos.sh <target-triple> <version> <arch>" >&2
    exit 2
fi

triple="$1"
version="$2"
arch="$3"

if [[ ! "$version" =~ ^[0-9]+(\.[0-9]+)*$ ]]; then
    echo "bundle-macos: version must be numeric dotted, got '$version'" >&2
    exit 2
fi
if [[ ! "$arch" =~ ^[A-Za-z0-9_]+$ ]]; then
    echo "bundle-macos: arch must be a plain identifier, got '$arch'" >&2
    exit 2
fi

binary="$root/target/$triple/release/bromine"
if [[ ! -x "$binary" ]]; then
    echo "bundle-macos: no release binary at $binary" >&2
    echo "build it first: cargo build --release --target $triple -p bromine" >&2
    exit 1
fi
if [[ ! -f "$root/assets/AppIcon.icns" ]]; then
    echo "bundle-macos: missing assets/AppIcon.icns; run scripts/make-icon.sh" >&2
    exit 1
fi

identity="${SIGN_IDENTITY:--}"
app="$root/dist/Bromine.app"
dmg="$root/dist/Bromine-$version-$arch.dmg"
staging="$(mktemp -d "${TMPDIR:-/tmp}/bromine-bundle.XXXXXX")"
trap 'rm -rf "$staging"' EXIT

rm -rf "$app" "$dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$root/dist"

cp "$binary" "$app/Contents/MacOS/bromine"
chmod 755 "$app/Contents/MacOS/bromine"
cp "$root/assets/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
cp "$root/packaging/Info.plist" "$app/Contents/Info.plist"

# Stamp the template rather than shipping placeholders in the bundle. `--build`
# uses the version here: it is a legal CFBundleVersion and this project has no
# separate build number to keep in sync.
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

if [[ "$identity" == "-" ]]; then
    codesign --force --deep --sign - "$app"
else
    codesign --force --deep --sign "$identity" --options runtime --timestamp "$app"
fi
codesign -dv "$app" 2>&1
codesign --verify --deep --strict "$app"

# A dmg with the app and an /Applications symlink is the drag-to-install
# gesture macOS users know. ditto preserves the signature's extended attributes
# that a plain cp can drop.
ditto "$app" "$staging/Bromine.app"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "Bromine" -srcfolder "$staging" -ov -format UDZO "$dmg" >/dev/null

echo "app: $app"
echo "dmg: $dmg"
echo "sha256: $(shasum -a 256 "$dmg" | awk '{print $1}')"
