//! Long-form CLI help, modelled on Shaka Packager's gflags descriptions.
//!
//! Short help (`-h`) lives in the `///` comments on each flag. `--help` uses
//! these paragraphs. Analogous Shaka flags are named so users translating a
//! `packager` command can find the sheathe equivalent.

/// Root `sheathe --help` body.
pub(crate) const ROOT_LONG_ABOUT: &str = "\
Pure-Rust HLS / DASH / CMAF media packager — a memory-safe, dependency-light \
alternative to Shaka Packager's `packager` binary.

Positional arguments are media paths **or** Shaka stream descriptors \
(`in=file,stream=audio,language=eng,drm_label=AUDIO,…`). A bare path packages \
every track in the file. Multiple inputs form an ABR ladder unless \
`--multi-period` is set.

Commands:
  package   Demux → fragment → CMAF / MPEG-TS / packed-audio segments + DASH/HLS
  probe     Dump demuxed stream info (Shaka `--dump_stream_info`) without packaging
  origin    JIT HTTP origin: package on GET /package?input=…

Pass `-h` for a compact flag list, `--help` for these descriptions. Full \
reference: https://github.com/vbasky/sheathe/blob/main/docs/CLI.md";

/// Examples under `sheathe --help`.
pub(crate) const ROOT_AFTER_HELP: &str = "\
Examples:
  sheathe package movie.mp4 -o site/ --dash --hls --segment-duration 6
  sheathe package v360.mp4 v720.mp4 v1080.mp4 -o ladder/ --dash --hls
  sheathe probe movie.mp4
  sheathe origin --bind 127.0.0.1:8787 --media-root .

Shaka Packager equivalent of the first example:
  packager \\
    in=movie.mp4,stream=audio,init_segment=site/audio.mp4,segment_template=site/audio_$Number$.m4s \\
    in=movie.mp4,stream=video,init_segment=site/video.mp4,segment_template=site/video_$Number$.m4s \\
    --segment_duration 6 --mpd_output site/manifest.mpd \\
    --hls_master_playlist_output site/master.m3u8";

pub(crate) const PACKAGE_LONG_ABOUT: &str = "\
Package one or more inputs into media segments and optional DASH / HLS manifests.

Each positional argument is a media path **or** a Shaka stream descriptor \
(`in=file,stream=audio|video|text|N,…`). Format is auto-detected. \
CEA-608/708 captions embedded in H.264/H.265 SEI are extracted to WebVTT tracks.

Multiple inputs form an ABR ladder by default (all tracks share one DASH Period \
and one HLS master). With `--multi-period`, each file is a successive DASH Period.

`--dash` and `--hls` are independent opt-in flags (Shaka `--mpd_output` / \
`--hls_master_playlist_output`). Pass at least one, or sheathe writes only \
segment files.

Output names are assigned by sheathe (`init_N.mp4`, `seg_N_M.m4s`, \
`media_N.m3u8`, `master.m3u8`, `manifest.mpd`) rather than per-stream \
`output=` / `segment_template=` descriptors.";

pub(crate) const PACKAGE_AFTER_HELP: &str = "\
Examples:
  sheathe package movie.mp4 -o site/ --dash --hls --segment-duration 6
  sheathe package v360.mp4 v720.mp4 v1080.mp4 -o ladder/ --dash --hls
  sheathe package intro.mp4 main.mp4 -o show/ --dash --multi-period
  sheathe package in.mp4 -o live/ --dash --hls --presentation live --live-window 3
  sheathe package in.mp4 -o drm/ --dash --hls \\
    --enc-key 00112233445566778899aabbccddeeff:000102030405060708090a0b0c0d0e0f \\
    --protection-systems common,widevine,playready
  sheathe package in.mp4 -o ts/ --hls --format ts
  sheathe package in.mp4 -o od/ --dash --on-demand

Shaka flag map (common cases):
  stream_descriptor input= / in=     positional <INPUTS>
  output= / init_segment=            -o / --out  (sheathe names files)
  segment_template=                  -o / --out  (sheathe names files)
  --segment_duration                 --segment-duration
  --mpd_output                       --dash
  --hls_master_playlist_output       --hls
  --hls_playlist_type                --presentation
  --availability_start_time          --availability-start-time
  trick_play_factor=                 --trick-play
  --ad_cues                          --scte35
  --enable_raw_key_encryption --keys --enc-key / --enc-key-file
  --protection_scheme                --enc-scheme
  --hls_key_uri                      --enc-key-uri
  --protection_systems               --protection-systems
  --crypto_period_duration           --crypto-period-duration
  --low_latency_dash_mode            --low-latency
  http:// output URLs                --http-push";

pub(crate) const PROBE_LONG_ABOUT: &str = "\
Inspect an input and print the streams sheathe would package, without writing \
any output.

This is the sheathe equivalent of Shaka Packager `--dump_stream_info`. Shaka \
still requires a stream_descriptor to run; `sheathe probe` is inspect-only.

The dump lists container, size, and per-stream type, codec, RFC 6381 \
codec_string, timescale, duration, sample count, resolution / sample rate, \
and average bitrate — the same fields Shaka prints when they are known.";

pub(crate) const PROBE_AFTER_HELP: &str = "\
Examples:
  sheathe probe movie.mp4
  sheathe probe broadcast.ts
  sheathe probe clip.webm

Shaka Packager equivalent:
  packager in=movie.mp4,stream=0,output=/dev/null --dump_stream_info";

pub(crate) const ORIGIN_LONG_ABOUT: &str = "\
Run a JIT HTTP/1.1 origin: package media on demand and serve manifests and \
cached segments.

`--tls-cert` + `--tls-key` wrap the accept loop in rustls (HTTPS). `--auth \
user:password` requires HTTP Basic. This is still a development origin, not a \
CDN — no cache eviction, no multi-tenant isolation.

There is no Shaka Packager equivalent.

HTTP API:
  GET /health  or  /healthz
      200 plain `ok`
  GET /package?input=<path>&format=hls|dash|both
      Master playlist or MPD. Packages into --cache-dir on first request.
  GET /out/<cache-rel>
      Cached object (segments, playlists, init).

`input` may be relative to `--media-root` (path-traversal protected) or \
absolute when the sandbox allows it.";

pub(crate) const ORIGIN_AFTER_HELP: &str = "\
Examples:
  sheathe origin --bind 127.0.0.1:8787 --media-root ./corpus/media
  curl -s 'http://127.0.0.1:8787/package?input=bear-1280x720.mp4&format=hls'
  curl -s http://127.0.0.1:8787/health
  sheathe origin --bind 127.0.0.1:8443 --tls-cert cert.pem --tls-key key.pem --auth user:pass
  curl -sk https://127.0.0.1:8443/health";

pub(crate) const NO_BANNER: &str = "\
Suppress the startup ASCII / inline-image banner that sheathe prints to \
stderr before running a command. Useful in scripts and CI logs. Sheathe-only; \
Shaka Packager has no banner.";

pub(crate) const INPUTS: &str = "\
Input media file path(s). Each file is demuxed and every audio, video, and \
text track becomes a rendition.

Unlike Shaka Packager stream_descriptors, you do not select `stream=audio` / \
`stream=video` / a stream number — all tracks are packaged. Pass several files \
to build an ABR ladder (one rendition set per file). With `--multi-period`, \
each file is a successive DASH Period instead of a ladder rung.

Supported containers (auto-detected): MP4 / fMP4 / CMAF, MPEG-TS, WebM / \
Matroska, raw elementary streams (.h264 / .h265 / .aac / .ac3 / …), WebVTT, \
TTML / IMSC.";

pub(crate) const OUT: &str = "\
Output directory (created if missing). Default: `out`.

Sheathe writes init segments, media segments, and manifests here using its \
own names (`init_N.mp4`, `seg_N_M.m4s`, `media_N.m3u8`, `master.m3u8`, \
`manifest.mpd`). This replaces Shaka's per-stream `output=` / `init_segment=` \
/ `segment_template=` plus `--mpd_output` and `--hls_master_playlist_output`.";

pub(crate) const SEGMENT_DURATION: &str = "\
Target segment duration in seconds. Default: 6.

Segments are cut on keyframes, so actual durations may not match the request \
exactly — the same caveat as Shaka `--segment_duration`. With `--on-demand`, \
this is the subsegment duration inside the single-file Representation. With \
`--low-latency`, parts of `--part-duration` are generated inside each segment.";

pub(crate) const DASH: &str = "\
Write a DASH Media Presentation Description as `manifest.mpd` in the output \
directory.

Equivalent to Shaka `--mpd_output <dir>/manifest.mpd`. Independent of `--hls`; \
if neither `--dash` nor `--hls` is set, only segment files are written. \
On-demand profile (`isoff-on-demand` + SegmentList byte ranges) additionally \
requires `--on-demand`.";

pub(crate) const HLS: &str = "\
Write an HLS master playlist `master.m3u8` plus per-track media playlists \
`media_N.m3u8` (and `iframe_N.m3u8` when `--trick-play` is set).

Equivalent to Shaka `--hls_master_playlist_output <dir>/master.m3u8`. \
Independent of `--dash`. Playlist type follows `--presentation` (VOD / EVENT \
/ LIVE), matching Shaka `--hls_playlist_type`.";

pub(crate) const PRESENTATION: &str = "\
How the presentation is signalled in DASH and HLS. Default: `vod`.

  vod    DASH type=\"static\" with mediaPresentationDuration; HLS \
#EXT-X-PLAYLIST-TYPE:VOD and #EXT-X-ENDLIST. Finished asset.
  event  DASH type=\"dynamic\" without a sliding window; HLS \
#EXT-X-PLAYLIST-TYPE:EVENT and no ENDLIST. Growing event.
  live   DASH type=\"dynamic\" with a sliding window, UTCTiming, and live \
attributes; HLS omits PLAYLIST-TYPE and ENDLIST, sets #EXT-X-MEDIA-SEQUENCE. \
Window size is `--live-window` (default 3).

Maps to Shaka `--hls_playlist_type` (VOD / EVENT / LIVE) plus DASH static vs \
dynamic generation. Live packaging is VOD-as-live: a finished file is described \
with live tags. Continuous rewrite is `sheathe origin`.";

pub(crate) const LIVE_WINDOW: &str = "\
Live / event sliding-window size in segments. Default: 3 for `--presentation \
live`; no window (keep every segment) for event/VOD unless this flag is set.

The last N segments are kept in the MPD SegmentTemplate / HLS media playlist. \
Analogous to Shaka `--preserved_segments_outside_live_window` and the DASH \
time-shift buffer depth derived from it.";

pub(crate) const MULTI_PERIOD: &str = "\
Treat each input file as a successive DASH Period instead of an ABR ladder \
rung. Period `@start` offsets are contiguous across files.

Without this flag (the default), every input's tracks share one Period / one \
HLS master as alternate renditions. Shaka typically builds multi-period MPDs \
via cue points (`--ad_cues`) or by concatenating packager runs; sheathe takes \
one file per Period.";

pub(crate) const TRICK_PLAY: &str = "\
Emit trick-play (I-frame / trick-mode) tracks for each video rendition.

Writes keyframe-only CMAF segments, a DASH trick AdaptationSet \
(EssentialProperty + maxPlayoutRate), and HLS #EXT-X-I-FRAME-STREAM-INF / \
#EXT-X-I-FRAMES-ONLY playlists (`iframe_N.m3u8`).

Shaka equivalent: stream_descriptor `trick_play_factor=` (tpf) plus \
`iframe_playlist_name=`.";

pub(crate) const LOW_LATENCY: &str = "\
Low-latency packaging: LL-HLS partial segments and LL-DASH availability \
offset.

HLS media playlists gain EXT-X-PART / PART-INF / SERVER-CONTROL / \
PRELOAD-HINT. DASH gets availabilityTimeOffset and \
availabilityTimeComplete=\"false\". Part length is `--part-duration` \
(default 1s).

Shaka equivalent: `--low_latency_dash_mode`. Combine with `--presentation \
live` for a live-style low-latency manifest.";

pub(crate) const PART_DURATION: &str = "\
Part target duration in seconds when `--low-latency` is set. Default: 1.

Parts are budgeted by sample duration inside each media segment. Ignored \
unless `--low-latency` is also passed. Must be positive.";

pub(crate) const AVAILABILITY_START_TIME: &str = "\
Wall-clock availability start as ISO-8601 UTC, e.g. `2026-07-18T00:00:00Z`.

Written as DASH MPD@availabilityStartTime and used as the origin for HLS \
#EXT-X-PROGRAM-DATE-TIME when live/event. Defaults to the current time for \
`--presentation live` or `event`. Ignored for VOD.

Shaka equivalent: `--availability_start_time`.";

pub(crate) const SCTE35: &str = "\
SCTE-35 splice marker. Repeatable. Format: TIME[:out|in][:BREAK_DUR]

  TIME        seconds from period start (required, non-negative)
  out|in      cue-out (default) or cue-in (return to network)
  BREAK_DUR   planned break duration in seconds (optional)

Examples: `--scte35 30`  `--scte35 30:out:15`  `--scte35 45:in`

DASH: EventStream schemeIdUri=\"urn:scte:scte35:2014:xml+bin\" (base64 \
splice_insert). HLS: #EXT-X-DATERANGE with SCTE35-OUT / SCTE35-IN (hex 0x…).

Shaka equivalent: `--ad_cues {start}[,{duration}][;…]` (cue-out only; sheathe \
also has explicit cue-in).";

pub(crate) const ENC_KEY: &str = "\
Encrypt with a raw key given on the command line as `<KID hex>:<KEY hex>`. \
Both values are 16 bytes (32 hex characters).

Enables Common Encryption on every stream. Prefer `--enc-key-file` so the key \
does not appear in process arguments or shell history.

Shaka equivalent: `--enable_raw_key_encryption --keys \
key_id=<KID>:key=<KEY>` (Shaka also supports per-stream `drm_label`). Sheathe \
applies one key to all tracks.";

pub(crate) const ENC_KEY_FILE: &str = "\
Read the raw key from a file instead of `--enc-key`. The file may contain \
comments (`#` to end of line) and blank lines; the first `<KID hex>:<KEY hex>` \
token is used.

Takes precedence over `--enc-key`. This is the recommended way to pass keys.";

pub(crate) const ENC_SCHEME: &str = "\
Common Encryption scheme when a key is set. Default: `cenc`.

  cenc   AES-128-CTR, full-sample (ISO/IEC 23001-7)
  cens   AES-128-CTR pattern (typically 1:9 crypt:skip)
  cbc1   AES-128-CBC, full-sample
  cbcs   AES-128-CBC pattern (HLS SAMPLE-AES)

Shaka equivalent: `--protection_scheme`. HLS #EXT-X-KEY METHOD is \
SAMPLE-AES-CTR for cenc/cens and SAMPLE-AES for cbcs/cbc1. Pattern schemes \
apply to video; audio is typically fully encrypted.";

pub(crate) const ENC_KEY_URI: &str = "\
Key-delivery URI written into the HLS #EXT-X-KEY tag when encrypting. \
Default: `key.bin`.

Ignored when not encrypting. Does not fetch or write the key; it is only \
signalled in the playlist. Shaka equivalent: `--hls_key_uri` (identity / \
streamingkeydelivery formats).";

pub(crate) const PROTECTION_SYSTEMS: &str = "\
Comma-separated DRM systems to emit `pssh` boxes (and DASH ContentProtection \
/ HLS signalling) for. Default: `common`.

  common      Common PSSH (system id 1077efec-…)
  widevine    Widevine PSSH
  playready   PlayReady PSSH (WRMHEADER)

Example: `--protection-systems common,widevine,playready`

Shaka equivalent: `--protection_systems`. Additional Shaka key sources \
(Widevine license server, CPIX/SPEKE) are not implemented; use raw keys.";

pub(crate) const CRYPTO_PERIOD_DURATION: &str = "\
Enable key rotation with this crypto-period duration in seconds. Each period \
derives a key from the base `--enc-key` / `--enc-key-file` key; rotation is \
signalled per segment via `seig` sample groups and per-period `pssh`.

Must be positive. Omitted (the default) means a single key for the whole \
presentation. Shaka equivalent: `--crypto_period_duration`.";

pub(crate) const FORMAT: &str = "\
Segment container. Default: `cmaf`.

  cmaf           Fragmented MP4 init (`init_N.mp4`) + `.m4s` media. DASH + \
fMP4 HLS (#EXT-X-MAP). Shaka default MP4/CMAF path.
  ts             MPEG-TS segments (`.ts`) with PAT/PMT/PES. HLS without \
#EXT-X-MAP. Shaka MPEG2-TS muxer / `output_format`.
  packed-audio   Raw ADTS-AAC / AC-3 elementary bursts. Audio-only HLS.

`--on-demand` applies only to CMAF + DASH (concatenated `rep_N.mp4`).";

pub(crate) const ON_DEMAND: &str = "\
DASH on-demand single-file output.

Concatenates each rendition into `rep_N.mp4` and describes it with the \
`isoff-on-demand` profile, BaseURL, and SegmentList `@mediaRange` byte \
ranges instead of a SegmentTemplate of `.m4s` files.

Requires `--dash`. Analogous to running Shaka without `segment_template=` \
(single-file output) plus a static on-demand MPD. Ignored for `--format ts` \
/ `packed-audio`.";

pub(crate) const PARALLEL: &str = "\
Package tracks concurrently using std threads (one task per track).

Speeds up multi-rendition / multi-track inputs. Deterministic file names are \
preserved. Shaka pipelines internally; it has no equivalent user-facing flag. \
Errors from any track fail the run.";

pub(crate) const HTTP_PUSH: &str = "\
After writing locally, HTTP/1.1 PUT every object (init, media, playlists, \
MPD) to this base URL as `{URL}/{filename}`.

Plain HTTP only — terminate TLS at a proxy. Shaka can take `http://` / \
`https://` paths directly as `output=` / `segment_template=`; sheathe always \
writes locally first, then pushes. Failures abort the run \
(Shaka `--ignore_http_output_failures` is not implemented).";

pub(crate) const ORIGIN_BIND: &str = "\
Listen address for the origin HTTP(S) server. Default: `127.0.0.1:8787`.

Use `0.0.0.0:8787` to accept non-local clients. Pair with `--tls-cert` / \
`--tls-key` for HTTPS, or put a TLS proxy in front.";

pub(crate) const ORIGIN_MEDIA_ROOT: &str = "\
Sandbox root for relative `input=` query paths. Default: the current \
directory.

Paths are rejected if they escape this directory (no `..` traversal). \
Absolute paths are subject to the same sandbox.";

pub(crate) const ORIGIN_CACHE_DIR: &str = "\
Directory for packaged outputs served from `/out/…`. Default: \
`/tmp/sheathe-origin`.

A cache hit on a previously packaged `input` + format skips re-packaging. \
Not a production CDN cache (no eviction policy beyond the filesystem).";

pub(crate) const ORIGIN_SEGMENT_DURATION: &str = "\
Default `--segment-duration` for JIT `GET /package` runs. Default: 6.

Overridable only here for the origin process, not per request.";
