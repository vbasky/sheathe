#!/usr/bin/env bash
# Fetch a real-media test corpus for the oracle harness (see corpus/manifest.toml).
#
# The media blobs are test-licensed and NOT committed (see .gitignore); only this
# script, manifest.toml, and checksums.sha256 are version-controlled. Run:
#
#     ./corpus/fetch.sh            # download + verify against pinned checksums
#     ./corpus/fetch.sh --update   # re-pin checksums.sha256 from what was fetched
#
# Sources:
#   - Chromium media/test/data — tiny, version-pinned files covering the Phase 3
#     container/codec matrix (TS, WebM, AV1, Opus, AC-3, E-AC-3, FLAC, MP3, Ogg).
#   - Apple BipBop advanced — real in-band CEA-608 captions in an H.264 TS.
#   - Raw elementary streams derived locally from the TS with ffmpeg.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MEDIA="$ROOT/corpus/media"
CHECKSUMS="$ROOT/corpus/checksums.sha256"
UPDATE=${1:-}
mkdir -p "$MEDIA"

CHROMIUM="https://chromium.googlesource.com/chromium/src/+/refs/heads/main/media/test/data"
# Directly downloaded, checksum-gated originals.
CHROMIUM_FILES=(
  bear-1280x720.ts        # MPEG-TS   : H.264 + AAC          -> sheathe-ts
  bear-1280x720.mp4       # MP4       : H.264 + AAC          -> sheathe-mp4 (baseline)
  bear-vp9.webm           # WebM      : VP9                  -> sheathe-mkv
  bear-vp8-webvtt.webm    # WebM      : VP8 + WebVTT         -> sheathe-mkv / text
  bear-av1.mp4            # MP4       : AV1                  -> sheathe-mp4 (av01)
  bear-opus.webm          # WebM      : Opus                 -> sheathe-mkv (Opus/dOps)
  bear-vp9-opus.webm      # WebM      : VP9 + Opus           -> sheathe-mkv
  bear-flac.mp4           # MP4       : FLAC                 -> sheathe-mp4 (dfLa)
  bear-ac3-only-frag.mp4  # fMP4      : AC-3                 -> sheathe-mp4 (dac3)
  bear-eac3-only-frag.mp4 # fMP4      : E-AC-3               -> sheathe-mp4 (dec3)
  sfx.mp3                 # MP3       : mp4a OTI 0x6b        -> sheathe-mp4
  sfx.flac                # raw FLAC                          -> sheathe-es (future)
  sfx.ogg                 # Ogg       : Vorbis               -> sheathe-mkv (Vorbis, open)
)

BIPBOP="https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_16x9/bipbop_16x9_variant.m3u8"

have() { command -v "$1" >/dev/null 2>&1; }
sha256() { if have sha256sum; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
# gitiles ?format=TEXT returns the whole file as one unbroken base64 line, which
# the base64/openssl CLIs mishandle; Python decodes it reliably.
b64d() { python3 -c "import base64,sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.buffer.read()))"; }

if ! have python3; then echo "python3 required (base64 decode)" >&2; exit 1; fi

echo "→ Chromium media test data"
for f in "${CHROMIUM_FILES[@]}"; do
  out="$MEDIA/$f"
  if [[ -s $out ]]; then echo "  cached $f"; continue; fi
  printf "  fetch  %s ... " "$f"
  if curl -sS -L --fail --max-time 120 "$CHROMIUM/$f?format=TEXT" | b64d > "$out" 2>/dev/null && [[ -s $out ]]; then
    echo "ok"
  else
    echo "FAILED"; rm -f "$out"
  fi
done

echo "→ Apple BipBop (CEA-608 in SEI)"
if have ffmpeg; then
  if [[ -s "$MEDIA/bipbop-cea608.ts" ]]; then
    echo "  cached bipbop-cea608.ts"
  elif ffmpeg -y -loglevel error -i "$BIPBOP" -t 30 -map 0:v:0 -c copy "$MEDIA/bipbop-cea608.ts" 2>/dev/null; then
    echo "  fetched bipbop-cea608.ts"
  else
    echo "  ⚠ ffmpeg HLS pull failed — caption sample skipped" >&2
  fi
else
  echo "  ⚠ ffmpeg not found — caption sample skipped" >&2
fi

echo "→ derive raw elementary streams"
if have ffmpeg && [[ -s "$MEDIA/bear-1280x720.ts" ]]; then
  [[ -s "$MEDIA/bear.h264" ]] || ffmpeg -y -loglevel error -i "$MEDIA/bear-1280x720.ts" -map 0:v:0 -c copy -f h264 "$MEDIA/bear.h264" 2>/dev/null || true
  [[ -s "$MEDIA/bear.aac"  ]] || ffmpeg -y -loglevel error -i "$MEDIA/bear-1280x720.ts" -map 0:a:0 -c copy -f adts "$MEDIA/bear.aac" 2>/dev/null || true
  echo "  derived bear.h264, bear.aac"
fi
if have ffmpeg && [[ -s "$MEDIA/bear-ac3-only-frag.mp4" ]]; then
  [[ -s "$MEDIA/bear.ac3" ]] || ffmpeg -y -loglevel error -i "$MEDIA/bear-ac3-only-frag.mp4" -map 0:a:0 -c copy -f ac3 "$MEDIA/bear.ac3" 2>/dev/null || true
  echo "  derived bear.ac3"
fi

# Checksums gate only the directly downloaded originals (derived/HLS-muxed files
# vary by ffmpeg version, so they are not pinned).
echo "→ checksums (downloaded originals)"
cd "$MEDIA"
present=(); for f in "${CHROMIUM_FILES[@]}"; do [[ -s $f ]] && present+=("$f"); done
if [[ ${#present[@]} -eq 0 ]]; then
  echo "  ✗ nothing downloaded" >&2; exit 1
fi
if [[ -f $CHECKSUMS && $UPDATE != "--update" ]]; then
  if sha256 -c <(grep -E "  ($(IFS='|'; echo "${present[*]}"))\$" "$CHECKSUMS") ; then
    echo "  ✓ checksums verified (${#present[@]} files)"
  else
    echo "  ✗ checksum mismatch — upstream content changed; re-pin with --update after review" >&2
    exit 1
  fi
else
  sha256 "${present[@]}" > "$CHECKSUMS"
  echo "  wrote $(basename "$CHECKSUMS") (${#present[@]} files) — commit it to pin the corpus"
fi

echo "done — corpus in corpus/media/ ($(ls -1 "$MEDIA" | wc -l | tr -d ' ') files)"
