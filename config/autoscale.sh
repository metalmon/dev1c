#!/bin/sh
# Pick an interface scale to match the RDP session resolution.
#
# Why: the resolution is chosen by the client, is unknown in advance and changes
# from one connection to the next. A fixed DPI of 96 makes the interface
# unreadably small on 4K, while a large DPI on 1366x768 leaves nothing fitting on
# screen.
#
# The physical screen size is unavailable in an xrdp session (xrandr reports
# 0mm x 0mm), so DPI cannot be computed honestly — width in pixels is the guide.
#
# ponytail: thresholds picked from common resolutions, no clever maths. For
# manual tuning, FORCE_DPI overrides the calculation.
set -eu

WIDTH=$(xrandr --query 2>/dev/null | sed -n 's/^Screen 0:.*current \([0-9]\+\) x.*/\1/p')
[ -n "${WIDTH:-}" ] || WIDTH=1920

if [ -n "${FORCE_DPI:-}" ]; then
    DPI="$FORCE_DPI"
    SCALE=1
elif [ "$WIDTH" -ge 3400 ]; then
    # 4K and above: integer doubling, otherwise the interface is unreadable
    DPI=96
    SCALE=2
elif [ "$WIDTH" -ge 2400 ]; then
    # 2560x1440 and similar: 1.25x through fonts, without doubling the widgets
    DPI=120
    SCALE=1
else
    DPI=96
    SCALE=1
fi

printf 'Xft.dpi: %s\n' "$DPI" | xrdb -merge 2>/dev/null || true

export GDK_SCALE="$SCALE"
if [ "$SCALE" -gt 1 ]; then
    # Compensation: otherwise fonts are scaled twice, by GDK_SCALE and by DPI
    export GDK_DPI_SCALE=0.5
fi

echo "autoscale: width=${WIDTH} dpi=${DPI} scale=${SCALE}" >&2
