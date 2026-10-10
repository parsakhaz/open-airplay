#!/usr/bin/env bash
# Stages a self-contained receiver runtime for the installer: engine/runtime/
#   bin/uxplay.exe + every MSYS2 DLL it and the staged plugins need
#   lib/gstreamer-1.0/<only the plugins UxPlay's pipelines use>
# GStreamer finds plugins relative to libgstreamer-1.0-0.dll (../lib/gstreamer-1.0), so the
# installed app needs no MSYS2, no PATH changes and no GST_PLUGIN_* variables.
# Run after build-engine.sh (it calls this script at the end).
set -euo pipefail

export MSYSTEM=UCRT64
export PATH="/ucrt64/bin:/usr/bin:$PATH"

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC_EXE="$HERE/uxplay-src/build/uxplay.exe"
OUT="$HERE/runtime"
PLUGIN_SRC="/ucrt64/lib/gstreamer-1.0"

# Elements used by UxPlay's default mirror + audio pipelines (see its "GStreamer ... pipeline" log lines):
#   appsrc (app), queue/typefind/identity (coreelements), typefind functions, h264parse/h265parse
#   (videoparsersbad), decodebin (playback), avdec_* (libav), d3d12/d3d11 sinks + hw decoders,
#   videoconvert/videoscale (videoconvertscale), autovideosink/autoaudiosink (autodetect),
#   wasapi sinks, audioconvert, audioresample, volume, level.
PLUGINS=(
  app coreelements typefindfunctions videoparsersbad playback libav
  d3d12 d3d11 videoconvertscale autodetect wasapi wasapi2 directsound
  audioconvert audioresample volume level
)

[ -f "$SRC_EXE" ] || { echo "missing $SRC_EXE: run build-engine.sh first" >&2; exit 1; }
rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/lib/gstreamer-1.0"
cp "$SRC_EXE" "$OUT/bin/"
for p in "${PLUGINS[@]}"; do
  cp "$PLUGIN_SRC/libgst$p.dll" "$OUT/lib/gstreamer-1.0/"
done

# Every MSYS2 DLL the exe and the plugins load, transitively (ldd resolves the whole tree).
{
  ldd "$OUT/bin/uxplay.exe"
  for f in "$OUT"/lib/gstreamer-1.0/*.dll; do ldd "$f"; done
} | awk '/\/ucrt64\// {print $3}' | sort -u | while read -r dll; do
  cp -n "$dll" "$OUT/bin/"
done

cp "$HERE/UXPLAY_PIN" "$OUT/UXPLAY_PIN"
echo "runtime staged: $OUT ($(du -sh "$OUT" | cut -f1), $(ls "$OUT/bin" | wc -l) files in bin, ${#PLUGINS[@]} plugins)"
