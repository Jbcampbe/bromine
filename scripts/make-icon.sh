#!/usr/bin/env bash
# Regenerate assets/AppIcon.icns from assets/icon.svg.
#
# The .icns is committed so CI and a clean checkout need no librsvg install;
# this script exists for when the artwork changes. It needs `rsvg-convert`
# (brew install librsvg), which `sips` and `iconutil` cannot replace for SVG
# input.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
svg="$root/assets/icon.svg"
master="$root/assets/icon-1024.png"
iconset="$root/assets/AppIcon.iconset"
icns="$root/assets/AppIcon.icns"

for tool in rsvg-convert sips iconutil; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "make-icon: missing $tool" >&2
        exit 1
    fi
done

# Every entry iconutil expects, including the @2x variants. Rendering each
# size from the same SVG keeps the Dock icon and the 16 px Finder icon the
# same drawing rather than a downscale of a downscale.
rm -rf "$iconset"
mkdir -p "$iconset"

rsvg-convert --width 1024 --height 1024 --output "$master" "$svg"

resize() { # <pixels> <file-name>
    sips --resampleHeightWidth "$1" "$1" "$master" --out "$iconset/$2" >/dev/null
}

resize 16 icon_16x16.png
resize 32 icon_16x16@2x.png
resize 32 icon_32x32.png
resize 64 icon_32x32@2x.png
resize 128 icon_128x128.png
resize 256 icon_128x128@2x.png
resize 256 icon_256x256.png
resize 512 icon_256x256@2x.png
resize 512 icon_512x512.png
resize 1024 icon_512x512@2x.png

iconutil --convert icns "$iconset" --output "$icns"

echo "wrote $icns"
echo "master: $master (inspect it before committing)"
