#!/usr/bin/env bash
# Differential harness: package the same input with sheathe and Shaka Packager,
# then diff CMAF segments and manifests. Requires `packager` on PATH (Shaka
# Packager v3.x). See ROADMAP.md — cross-cutting conformance.
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <input.mp4|input.ts> [segment_seconds]" >&2
  exit 1
fi

INPUT=$1
SEG=${2:-6}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

SHEATHE_OUT=$WORKDIR/sheathe
SHAKA_OUT=$WORKDIR/shaka
mkdir -p "$SHEATHE_OUT" "$SHAKA_OUT"

echo "→ sheathe package"
cargo run --quiet -p sheathe -- package "$INPUT" --out "$SHEATHE_OUT" --dash --hls --segment-duration "$SEG"

if ! command -v packager >/dev/null 2>&1; then
  echo "⚠ packager not on PATH — skipping Shaka oracle diff" >&2
  echo "  sheathe output: $SHEATHE_OUT" >&2
  exit 0
fi

echo "→ Shaka Packager"
packager \
  "in=$INPUT,stream=video,output=$SHAKA_OUT/video.mp4,segment_template=$SHAKA_OUT/video_\$Number\$.m4s" \
  "in=$INPUT,stream=audio,output=$SHAKA_OUT/audio.mp4,segment_template=$SHAKA_OUT/audio_\$Number\$.m4s" \
  --segment_duration "$SEG" \
  --mpd_output "$SHAKA_OUT/manifest.mpd" \
  --hls_master_playlist_output "$SHAKA_OUT/master.m3u8" \
  2>/dev/null || {
    echo "⚠ Shaka Packager failed (input may be TS-only or single-track) — compare manually" >&2
    exit 0
  }

echo "→ diff init/media segment counts"
echo "sheathe segments: $(find "$SHEATHE_OUT" -name '*.m4s' | wc -l | tr -d ' ')"
echo "shaka segments:   $(find "$SHAKA_OUT" -name '*.m4s' | wc -l | tr -d ' ')"

if command -v xmllint >/dev/null 2>&1; then
  if [[ -f $SHEATHE_OUT/manifest.mpd && -f $SHAKA_OUT/manifest.mpd ]]; then
    echo "→ canonical MPD diff (structure)"
    xmllint --c14n "$SHEATHE_OUT/manifest.mpd" > "$WORKDIR/sheathe.mpd.c14n" 2>/dev/null || true
    xmllint --c14n "$SHAKA_OUT/manifest.mpd" > "$WORKDIR/shaka.mpd.c14n" 2>/dev/null || true
    if diff -u "$WORKDIR/shaka.mpd.c14n" "$WORKDIR/sheathe.mpd.c14n"; then
      echo "✓ MPD canonical forms match"
    else
      echo "✗ MPD differs (expected during Phase 3+ — track deltas in ROADMAP)"
    fi
  fi
fi

echo "done — outputs in $WORKDIR (cleaned on exit)"