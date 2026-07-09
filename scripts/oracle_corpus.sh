#!/usr/bin/env bash
# Oracle-corpus runner: package every asset in corpus/manifest.toml with sheathe
# and check it against the per-asset oracle, then print a status table.
#
#   ./scripts/oracle_corpus.sh                # all assets
#   ./scripts/oracle_corpus.sh bear-vp9.webm  # one asset (substring match)
#
# The gate is decodability: sheathe's CMAF output (init + first segment) must be
# read back and decoded by ffprobe as the expected codec. Shaka Packager, when
# present, runs the same input as a *supplementary* cross-check (segment count is
# reported, not gated — packagers legitimately differ by a segment on boundaries).
# Captions are checked against ccextractor (a different oracle). Assets marked
# status="open" in the manifest are expected to fail (roadmap-open) and reported
# as XFAIL, which does not break green.
#
# Result codes:
#   OK     packaged + decodes as the expected codec (green)
#   MATCH  caption cues match the reference (green)
#   DELTA  packaged but output not decodable / wrong codec (work item)
#   EMPTY  packaged but produced 0 media segments
#   FAIL   sheathe errored or panicked before producing output
#   XFAIL  expected failure on a roadmap-open asset (does not break green)
#   SKIP   oracle tool unavailable (ccextractor / packager not on PATH)
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MEDIA="$ROOT/corpus/media"
MANIFEST="$ROOT/corpus/manifest.toml"
FILTER=${1:-}
BIN="$ROOT/target/debug/sheathe"
SEG=2

have() { command -v "$1" >/dev/null 2>&1; }
have python3 || { echo "python3 required" >&2; exit 1; }
[[ -f $MANIFEST ]] || { echo "no manifest at $MANIFEST — run corpus/fetch.sh" >&2; exit 1; }

echo "→ build sheathe"
cargo build -q -p sheathe 2>&1 | tail -1 || { echo "build failed" >&2; exit 1; }

HAVE_SHAKA=no; have packager && HAVE_SHAKA=yes
HAVE_CCX=no;   have ccextractor && HAVE_CCX=yes
have ffprobe || { echo "ffprobe required" >&2; exit 1; }
echo "  oracles: shaka=$HAVE_SHAKA ccextractor=$HAVE_CCX ffprobe=yes"

# Fields are joined with US (\x1f), a non-whitespace delimiter, so `read` keeps
# empty columns (a tab IFS would collapse them and shift fields).
rows=$(python3 - "$MANIFEST" <<'PY'
import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))
for a in d.get("asset", []):
    print("\x1f".join(a.get(k, "") for k in
          ("file", "input", "oracle", "video", "audio", "captions", "status")))
PY
)

# Package + oracle-check one asset. Echoes "<CODE>\t<detail>".
classify() {
  local file=$1 oracle=$2 video=$3 audio=$4 captions=$5 wd=$6
  local sdir="$wd/sheathe" log rc
  log=$("$BIN" --no-banner package "$MEDIA/$file" --out "$sdir" --dash --hls \
        --segment-duration "$SEG" 2>&1); rc=$?
  if [[ $rc -ne 0 ]]; then
    if grep -q panicked <<<"$log"; then
      printf 'FAIL\tpanic: %s' "$(grep -m1 panicked <<<"$log" | sed 's/.*panicked at //' | cut -c1-55)"
    else
      printf 'FAIL\t%s' "$(grep -m1 -iE 'error|malformed|unsupported' <<<"$log" | sed 's#.*/##' | cut -c1-55)"
    fi
    return
  fi
  local nseg; nseg=$(find "$sdir" -name '*.m4s' 2>/dev/null | wc -l | tr -d ' ')
  [[ $nseg -eq 0 ]] && { printf 'EMPTY\tinit only, 0 media segments'; return; }

  if [[ $oracle == ccextractor ]]; then caption_check "$file" "$sdir" "$wd"; return; fi

  # Decode gate: for every track, join its init + first segment and ffprobe the
  # codec, so multi-track inputs (video+audio) are each verified.
  local got="" init t seg join="$wd/join.mp4"
  for init in "$sdir"/init_*.mp4; do
    [[ -e $init ]] || continue
    t=$(basename "$init" .mp4); t=${t#init_}
    seg=$(find "$sdir" -name "seg_${t}_*.m4s" | sort | head -1)
    [[ -n $seg ]] || continue
    cat "$init" "$seg" > "$join" 2>/dev/null
    got="$got,$(ffprobe -v error -show_entries stream=codec_name -of csv=p=0 "$join" 2>/dev/null | paste -sd, -)"
  done
  got=$(tr ',' '\n' <<<"$got" | grep -v '^$' | sort -u | paste -sd, -)
  [[ -z $got ]] && { printf 'DELTA\tsheathe output not decodable by ffprobe'; return; }
  for want in "$video" "$audio"; do
    [[ -n $want && ",$got," != *",$want,"* ]] && { printf 'DELTA\tcodec got[%s] want[%s]' "$got" "$want"; return; }
  done

  # Supplementary Shaka cross-check (informational).
  local extra=""
  if [[ $HAVE_SHAKA == yes ]]; then
    local kdir="$wd/shaka"; mkdir -p "$kdir"; local -a sa=()
    [[ -n $video ]] && sa+=("in=$MEDIA/$file,stream=video,output=$kdir/v.mp4,segment_template=$kdir/v_\$Number\$.m4s")
    [[ -n $audio ]] && sa+=("in=$MEDIA/$file,stream=audio,output=$kdir/a.mp4,segment_template=$kdir/a_\$Number\$.m4s")
    if [[ ${#sa[@]} -gt 0 ]] && packager "${sa[@]}" --segment_duration "$SEG" --mpd_output "$kdir/m.mpd" >/dev/null 2>&1; then
      extra=" [shaka:$(find "$kdir" -name '*.m4s' | wc -l | tr -d ' ')]"
    fi
  fi
  printf 'OK\t%s segs, decodes as %s%s' "$nseg" "$got" "$extra"
}

# Caption oracle: diff sheathe's emitted caption text vs ccextractor on the
# source. sheathe emits an ISO-14496-30 `wvtt` track; ffmpeg can't demux that,
# so cue text is read directly from the `payl` boxes. Styling/position (the
# on-screen whitespace ccextractor adds) is intentionally ignored here.
caption_check() {
  local file=$1 sdir=$2 wd=$3
  if [[ $HAVE_CCX == no ]]; then printf 'SKIP\tccextractor not installed (caption oracle)'; return; fi
  local ref="$wd/ref.vtt"
  ccextractor -out=webvtt -o "$ref" "$MEDIA/$file" >/dev/null 2>&1
  [[ -s $ref ]] || { printf 'SKIP\tccextractor produced no cues'; return; }
  python3 - "$sdir" "$ref" <<'PY'
import sys, re, glob, os
sdir, ref = sys.argv[1], sys.argv[2]
def boxes(d, tag):
    out, i = [], 0
    while True:
        j = d.find(tag, i)
        if j < 0: break
        size = int.from_bytes(d[j-4:j], "big")
        out.append(d[j+4:j-4+size].decode("utf-8", "replace").strip())
        i = j + 4
    return out
got, gotpos = [], []
for seg in sorted(glob.glob(os.path.join(sdir, "seg_*.m4s"))):
    d = open(seg, "rb").read()
    got += boxes(d, b"payl")           # cue text
    gotpos += boxes(d, b"sttg")        # cue settings (line:%)
# Compare at the word level: ccextractor emits one cue per line while sheathe
# groups multi-line cues, so line-set matching is meaningless. Styling/position
# is not text, so it drops out naturally.
from collections import Counter
words = lambda ts: re.findall(r"[A-Z0-9']+", " ".join(ts).upper())
reflines = [l for l in open(ref, encoding="utf-8")
            if l.strip() and not l.startswith("WEBVTT") and "-->" not in l and not l[0].isdigit()]
gc, rc = Counter(words(got)), Counter(words(reflines))
total = sum(rc.values())
matched = sum(min(gc[w], rc[w]) for w in rc)
ratio = matched / total if total else 0
# Positioning: compare the multiset of line settings vs ccextractor's.
refpos = Counter(re.findall(r"line:[0-9.]+%", open(ref, encoding="utf-8").read()))
gotp = Counter(m for s in gotpos for m in re.findall(r"line:[0-9.]+%", s))
pos_ok = refpos == gotp
if not gc:
    print("DELTA\tsheathe emitted 0 caption cues")
elif ratio < 0.95:
    print(f"DELTA\tonly {matched}/{total} caption words match")
elif not pos_ok:
    print(f"DELTA\ttext ok ({matched}/{total}) but line positions differ: {dict(gotp)} vs {dict(refpos)}")
else:
    print(f"MATCH\t{matched}/{total} words + {sum(refpos.values())} line positions match ccextractor")
PY
}

pass=0; delta=0; fail=0; skip=0; xfail=0
printf '\n%-7s %-24s %-12s %s\n' STATUS ASSET INPUT DETAIL
printf '%.0s─' {1..92}; echo
while IFS=$'\x1f' read -r file input oracle video audio captions status; do
  [[ -z $file ]] && continue
  [[ -n $FILTER && $file != *"$FILTER"* ]] && continue
  if [[ ! -f "$MEDIA/$file" ]]; then printf '%-7s %-24s %-12s %s\n' SKIP "$file" "$input" "not fetched"; ((skip++)); continue; fi
  wd=$(mktemp -d); res=$(classify "$file" "$oracle" "$video" "$audio" "$captions" "$wd"); rm -rf "$wd"
  code=${res%%$'\t'*}; detail=${res#*$'\t'}
  # roadmap-open assets: downgrade a real failure to expected-fail.
  if [[ $status == open && $code =~ ^(FAIL|EMPTY|DELTA)$ ]]; then
    code=XFAIL; detail="$detail (roadmap-open)"
  fi
  printf '%-7s %-24s %-12s %s\n' "$code" "$file" "$input" "$detail"
  case $code in
    OK|MATCH) ((pass++));;
    XFAIL) ((xfail++));;
    DELTA|EMPTY) ((delta++));;
    FAIL) ((fail++));;
    SKIP) ((skip++));;
  esac
done <<<"$rows"

printf '%.0s─' {1..92}; echo
printf 'summary: %d pass · %d xfail(open) · %d delta/empty · %d fail · %d skip\n' \
  "$pass" "$xfail" "$delta" "$fail" "$skip"
if [[ $delta -eq 0 && $fail -eq 0 && $skip -eq 0 ]]; then
  echo "GREEN — all supported assets pass; open items are expected-fail."
else
  echo "not green — $((delta + fail)) unresolved, $skip skipped (install missing oracle tools)."
fi
