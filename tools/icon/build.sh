#!/usr/bin/env bash
# Regenerates every raster icon from the SVG masters.
#
# The SVGs are the source; the .png and .ico are derived and should never be
# edited by hand. Needs rsvg-convert and ImageMagick.
#
#   tools/icon/build.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
icons="$root/src-tauri/icons"
work="$(mtemp=$(mktemp -d); echo "$mtemp")"
trap 'rm -rf "$work"' EXIT

render() { rsvg-convert -w "$2" -h "$2" "$1" -o "$work/$2.png"; }

# Above 24px the weave reads; below it, only the silhouette survives, so the
# small frames come from the simplified master.
for size in 32 48 64 128 256; do render "$icons/icon.svg" "$size"; done
for size in 16 20 24; do render "$icons/icon-small.svg" "$size"; done

rsvg-convert -w 512 -h 512 "$icons/icon.svg" -o "$icons/icon.png"
rsvg-convert -w 32 -h 32 "$icons/icon.svg" -o "$icons/32x32.png"
rsvg-convert -w 128 -h 128 "$icons/icon.svg" -o "$icons/128x128.png"
rsvg-convert -w 256 -h 256 "$icons/icon.svg" -o "$icons/128x128@2x.png"

magick "$work/16.png" "$work/20.png" "$work/24.png" "$work/32.png" \
       "$work/48.png" "$work/64.png" "$work/128.png" "$work/256.png" \
       "$icons/icon.ico"

rsvg-convert -w 960 "$root/assets/logo.svg" -o "$root/assets/logo.png"

echo "icones gerados:"
identify -format '  %f  %wx%h\n' "$icons/icon.ico" "$icons"/*.png "$root/assets/logo.png"
