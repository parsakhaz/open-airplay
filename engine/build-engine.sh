#!/usr/bin/env bash
# engine-build: builds the pinned UxPlay engine (receiver sidecar) on Windows.
# Run from PowerShell:  C:\msys64\usr\bin\bash.exe -lc "MSYSTEM=UCRT64 /c/path/to/engine/build-engine.sh"
# or from a UCRT64 MSYS2 shell:  ./engine/build-engine.sh
# UxPlay >= 1.74 ships its own mDNS responder, so Bonjour is NOT required.
set -euo pipefail

UXPLAY_REPO="https://github.com/FDH2/UxPlay"
UXPLAY_PIN="$(cat "$(dirname "$0")/UXPLAY_PIN")"
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/uxplay-src"
OUT="$HERE/dist"

export MSYSTEM=UCRT64
export PATH="/ucrt64/bin:/usr/bin:$PATH"

pacman -S --needed --noconfirm \
  git \
  mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-ninja mingw-w64-ucrt-x86_64-gcc \
  mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-openssl mingw-w64-ucrt-x86_64-libplist \
  mingw-w64-ucrt-x86_64-gstreamer mingw-w64-ucrt-x86_64-gst-plugins-base \
  mingw-w64-ucrt-x86_64-gst-plugins-good mingw-w64-ucrt-x86_64-gst-plugins-bad \
  mingw-w64-ucrt-x86_64-gst-libav

if [ ! -d "$SRC/.git" ]; then
  git clone "$UXPLAY_REPO" "$SRC"
fi
git -C "$SRC" fetch --quiet origin
git -C "$SRC" checkout --quiet --detach "$UXPLAY_PIN"

# Our patches (kept small and upstreamable) live in engine/patches/*.patch.
# --ignore-whitespace: the checkout may have CRLF line endings (Git for Windows autocrlf).
for p in "$HERE"/patches/*.patch; do
  [ -e "$p" ] || continue
  if git -C "$SRC" apply --ignore-whitespace --reverse --check "$p" 2>/dev/null; then
    echo "patch already applied: $(basename "$p")"
  else
    git -C "$SRC" apply --ignore-whitespace "$p"
    echo "patch applied: $(basename "$p")"
  fi
done

cmake -S "$SRC" -B "$SRC/build" -G Ninja -DCMAKE_BUILD_TYPE=Release
ninja -C "$SRC/build"

# Unit test for the mirror-window sizing maths (patch 0002).
gcc -Wall -I "$SRC/renderers" "$HERE/tests/test_window_fit.c" -o "$SRC/build/test_window_fit.exe"
"$SRC/build/test_window_fit.exe"

# Regression test: a maximized mirror window reappears for the next stream (runs on a private desktop).
gcc -Wall -I "$SRC/renderers" "$HERE/tests/test_window_reshow.c" "$SRC/renderers/oap_window.c"   $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-video-1.0) -luser32 -lgdi32 -o "$SRC/build/test_window_reshow.exe"
"$SRC/build/test_window_reshow.exe"

# Manual check for the mirror window without an iPhone: engine/dist/window_demo.exe
gcc -Wall -O1 -I "$SRC/renderers" "$HERE/tests/window_demo.c" "$SRC/renderers/oap_window.c"   $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-video-1.0) -luser32 -lgdi32 -o "$SRC/build/window_demo.exe"

# Stage the engine plus every MSYS2 DLL it links against.
rm -rf "$OUT"; mkdir -p "$OUT"
cp "$SRC/build/uxplay.exe" "$OUT/"
cp "$SRC/build/window_demo.exe" "$OUT/"
ldd "$OUT/uxplay.exe" | awk '/\/ucrt64\// {print $3}' | sort -u | xargs -r -I{} cp {} "$OUT/"
echo "$UXPLAY_PIN" > "$OUT/UXPLAY_PIN"
echo "engine built: $OUT/uxplay.exe (UxPlay $UXPLAY_PIN)"
"$OUT/uxplay.exe" -v 2>&1 | head -3 || true

# Self-contained runtime for the installer (engine/runtime, bundled by tauri.windows.conf.json).
"$HERE/stage-runtime.sh"
