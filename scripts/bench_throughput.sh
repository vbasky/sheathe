#!/usr/bin/env bash
# Throughput micro-benchmark: sheathe package vs Shaka Packager (if present).
#
# Usage:
#   ./scripts/bench_throughput.sh [input] [runs]
#
# Reports wall-clock seconds for a fixed package of `input` (default: corpus
# bear MP4). Shaka is optional — when missing, only sheathe is timed.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

INPUT="${1:-corpus/media/bear-1280x720.mp4}"
RUNS="${2:-5}"
SEG=2

if [[ ! -f "$INPUT" ]]; then
  echo "input not found: $INPUT (run: just corpus)" >&2
  exit 1
fi

cargo build -p sheathe --release --quiet
SHEATHE=target/release/sheathe

bench_sheathe() {
  local t0 t1
  t0=$(date +%s.%N)
  for _ in $(seq 1 "$RUNS"); do
    rm -rf /tmp/sheathe-bench-out
    "$SHEATHE" package --no-banner "$INPUT" -o /tmp/sheathe-bench-out \
      --dash --hls --segment-duration "$SEG" --parallel >/dev/null
  done
  t1=$(date +%s.%N)
  python3 - <<PY
t0, t1, runs = float("$t0"), float("$t1"), int("$RUNS")
print(f"sheathe:  { (t1-t0)/runs :.4f}s/run  ({runs} runs, parallel)")
PY
}

bench_shaka() {
  if ! command -v packager >/dev/null 2>&1; then
    echo "shaka:    (packager not on PATH — skip)"
    return
  fi
  local t0 t1
  t0=$(date +%s.%N)
  for _ in $(seq 1 "$RUNS"); do
    rm -rf /tmp/shaka-bench-out
    mkdir -p /tmp/shaka-bench-out
    packager \
      "in=${INPUT},stream=video,init_segment=/tmp/shaka-bench-out/v-init.mp4,segment_template=/tmp/shaka-bench-out/v-\$Number\$.m4s" \
      "in=${INPUT},stream=audio,init_segment=/tmp/shaka-bench-out/a-init.mp4,segment_template=/tmp/shaka-bench-out/a-\$Number\$.m4s" \
      --segment_duration "$SEG" \
      --mpd_output /tmp/shaka-bench-out/manifest.mpd \
      --hls_master_playlist_output /tmp/shaka-bench-out/master.m3u8 \
      >/dev/null 2>&1 || true
  done
  t1=$(date +%s.%N)
  python3 - <<PY
t0, t1, runs = float("$t0"), float("$t1"), int("$RUNS")
print(f"shaka:    { (t1-t0)/runs :.4f}s/run  ({runs} runs)")
PY
}

echo "bench: input=$INPUT runs=$RUNS segment_duration=${SEG}s"
bench_sheathe
bench_shaka
