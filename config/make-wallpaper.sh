#!/bin/sh
# Generate the branded wallpaper.
#
# No longer part of the build: the picture came out identical every time, so it
# was generated once into config/brand/wallpaper.jpg and the image simply copies
# that file. Keeping imagemagick and rsvg-convert around for a constant was not
# worth it.
#
# Run this by hand after a branding change, from a machine that has imagemagick
# and librsvg2-bin:
#
#   BRAND_DIR=config/brand ./config/make-wallpaper.sh config/brand/wallpaper.jpg
#
# OktaPlus palette: #0E9E82 accent, #16191F dark background.
# Geometry: the mark itself (an octagon) as a large watermark — the brand shape
# rather than arbitrary circles — plus diagonals in the accent colour.
set -eu

OUT="${1:-/usr/share/backgrounds/dev1c.jpg}"
BRAND_DIR="${BRAND_DIR:-/usr/share/oktaplus}"
W="${W:-1920}"
H="${H:-1080}"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# The watermark mark and the centred lockup — from SVG to PNG at the right size
rsvg-convert -w 900 -h 900 "$BRAND_DIR/oktaplus-mark-light.svg" -o "$TMP/mark.png"
rsvg-convert -w 560 "$BRAND_DIR/oktaplus-lockup-horizontal-dark.svg" -o "$TMP/lockup.png"

magick -size "${W}x${H}" radial-gradient:"#17443c"-"#0b0e12" \
  \( -size "${W}x${H}" xc:none -stroke "rgba(14,158,130,0.10)" -strokewidth 1 \
     -draw "line 0,0 1920,1080" \
     -draw "line 260,0 1920,860" \
     -draw "line 0,260 1660,1080" \
     -draw "line 520,0 1920,640" \) \
  -compose over -composite \
  \( "$TMP/mark.png" -alpha set -channel A -evaluate multiply 0.07 +channel \) \
  -gravity east -geometry -140-60 -compose over -composite \
  \( -size "${W}x${H}" xc:black -fill white \
     -draw "polygon 820,0 1360,0 460,1080 0,1080" \
     -blur 0x170 -evaluate multiply 0.10 \) \
  -compose screen -composite \
  \( "$TMP/lockup.png" -alpha set -channel A -evaluate multiply 0.92 +channel \) \
  -gravity center -geometry +0-40 -compose over -composite \
  -quality 92 "$OUT"

echo "wallpaper: $OUT ($(wc -c <"$OUT") bytes)"
