#!/usr/bin/env bash
# Generate icons/icon.icns from icons/icon.png (run on macOS; uses sips + iconutil).
set -euo pipefail
ICONDIR="$(cd "$(dirname "$0")/../apps/desktop/src-tauri/icons" && pwd)"
SRC="$ICONDIR/icon.png"
SET="$(mktemp -d)/icon.iconset"
mkdir -p "$SET"
for s in 16 32 64 128 256 512; do
  sips -z $s $s "$SRC" --out "$SET/icon_${s}x${s}.png" >/dev/null
  d=$((s*2))
  sips -z $d $d "$SRC" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$ICONDIR/icon.icns"
echo "wrote $ICONDIR/icon.icns"
