#!/usr/bin/env bash
# Stages what "Mirror screen" needs into the installer's runtime, next to the receiver engine:
#   engine/runtime/bin/open-airplay-sender.exe   the Go mirror sender (cmd/open-airplay), run with -stdio
#   engine/runtime/bin/ffmpeg.exe                MSYS2 UCRT64 FFmpeg, which has the ddagrab capture filter
#   + any DLL ffmpeg needs that the engine runtime doesn't already ship (a handful; most are shared)
# The app puts engine/bin first on the sender's PATH, which is how the sender finds ffmpeg.
# Run in an MSYS2 UCRT64 shell after stage-runtime.sh, once the sender is built with Windows Go
# (from the repo root, in PowerShell or Git Bash; Go misbehaves inside MSYS2 shells):
#   go build -trimpath -ldflags "-s -w" -o engine/runtime/bin/open-airplay-sender.exe ./cmd/open-airplay
set -euo pipefail

export MSYSTEM=UCRT64
export PATH="/ucrt64/bin:/usr/bin:$PATH"

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HERE/runtime/bin"
SENDER="$OUT/open-airplay-sender.exe"

[ -d "$OUT" ] || { echo "missing $OUT: run build-engine.sh (or stage-runtime.sh) first" >&2; exit 1; }

# 1. The sender, built beforehand (see above).
[ -f "$SENDER" ] || { echo "missing $SENDER: build it first (see the header of this script)" >&2; exit 1; }

# 2. FFmpeg with ddagrab (Desktop Duplication), from the same MSYS2 the engine runtime comes from.
[ -x /ucrt64/bin/ffmpeg.exe ] || pacman -S --needed --noconfirm mingw-w64-ucrt-x86_64-ffmpeg
/ucrt64/bin/ffmpeg.exe -hide_banner -h filter=ddagrab 2>/dev/null | grep -q "Desktop Duplication" \
  || { echo "/ucrt64/bin/ffmpeg.exe has no ddagrab filter" >&2; exit 1; }
cp /ucrt64/bin/ffmpeg.exe "$OUT/"
added=0
while read -r dll; do
  if [ ! -f "$OUT/$(basename "$dll")" ]; then
    cp "$dll" "$OUT/"
    added=$((added + 1))
  fi
done < <(ldd "$OUT/ffmpeg.exe" | awk '/\/ucrt64\// {print $3}' | sort -u)

# 3. Smoke test: the staged ffmpeg runs from the staged folder alone, and the sender speaks the protocol.
PATH="$OUT:/usr/bin" "$OUT/ffmpeg.exe" -hide_banner -h filter=ddagrab >/dev/null
hello="$(printf '' | "$SENDER" -stdio 2>/dev/null | head -1)"
case "$hello" in
  *'"type":"hello"'*) ;;
  *) echo "sender did not say hello on -stdio (got: $hello)" >&2; exit 1 ;;
esac

echo "mirror sender staged: $SENDER, ffmpeg.exe + $added extra DLLs"
