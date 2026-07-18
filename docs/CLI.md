# sheathe CLI reference

Complete documentation for the `sheathe` binary: every subcommand, flag, input
format, and typical workflow. For design status see [ROADMAP.md](../ROADMAP.md);
for oracle / conformance gates see [CONFORMANCE.md](./CONFORMANCE.md).

```sh
cargo install sheathe          # crates.io
sheathe --help
sheathe <COMMAND> --help
```

Global options apply to every subcommand:

| Flag | Description |
|------|-------------|
| `--no-banner` | Suppress the startup ASCII banner |
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version |

---

## Commands at a glance

| Command | Purpose |
|---------|---------|
| [`sheathe package`](#sheathe-package) | Demux → fragment → write segments + DASH/HLS manifests |
| [`sheathe probe`](#sheathe-probe) | List streams sheathe detects in a file |
| [`sheathe origin`](#sheathe-origin) | JIT HTTP origin: package on request |

---

## `sheathe package`

Package one or more inputs into media segments and optional DASH / HLS
manifests.

```text
sheathe package [OPTIONS] <INPUTS>...
```

### Synopsis examples

```sh
# VOD CMAF + DASH + HLS
sheathe package movie.mp4 -o site/ --dash --hls --segment-duration 6

# ABR ladder (each input = one video rendition)
sheathe package v360.mp4 v720.mp4 v1080.mp4 -o ladder/ --dash --hls

# Multi-period DASH (each input = successive Period)
sheathe package intro.mp4 main.mp4 credits.mp4 -o show/ --dash --multi-period

# Live-style manifests from a finite file
sheathe package live.mp4 -o edge/ --dash --hls \
  --presentation live --live-window 3 \
  --availability-start-time 2026-07-18T00:00:00Z

# Encrypted (cenc) with Widevine + PlayReady pssh
sheathe package in.mp4 -o secure/ --dash --hls \
  --enc-key 00112233445566778899aabbccddeeff:000102030405060708090a0b0c0d0e0f \
  --protection-systems common,widevine,playready

# On-demand single-file DASH
sheathe package in.mp4 -o od/ --dash --on-demand

# MPEG-TS HLS segments
sheathe package in.mp4 -o ts/ --hls --format ts

# Trick-play + low-latency + SCTE-35
sheathe package in.mp4 -o ll/ --dash --hls \
  --trick-play --low-latency --part-duration 0.5 \
  --scte35 30:out:15 --scte35 45:in

# Parallel tracks + HTTP PUT to an ingest base URL
sheathe package in.mp4 -o out/ --dash --hls --parallel \
  --http-push http://127.0.0.1:8080/live
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<INPUTS>...` | One or more media paths. Multiple files form an **ABR ladder** by default (all tracks share one DASH Period / HLS master). With `--multi-period`, each file is a successive DASH Period. |

Supported inputs (auto-detected):

| Format | Extensions / sniff | Notes |
|--------|-------------------|--------|
| MP4 / fMP4 / CMAF | `.mp4`, `.m4s`, … | Progressive + fragmented |
| MPEG-TS | `.ts`, `.m2ts` | PAT/PMT/PES |
| WebM / Matroska | `.webm`, `.mkv` | VP8/VP9/AV1, Opus |
| Raw ES | `.h264`, `.h265`, `.aac`, `.ac3`, … | Extension + content sniff |
| WebVTT | `.vtt` | Text track passthrough |
| TTML / IMSC | `.ttml`, `.xml` | `stpp` sample entry |

**CEA-608/708** captions embedded in H.264/H.265 SEI are auto-extracted to
WebVTT tracks when present.

### Output layout (typical CMAF VOD)

```text
out/
  init_0.mp4          # init segment (audio or first track)
  init_1.mp4          # init segment (video, …)
  seg_0_1.m4s         # media segments
  seg_0_2.m4s
  seg_1_1.m4s
  …
  media_0.m3u8        # per-rendition HLS media playlist
  media_1.m3u8
  iframe_1.m3u8       # only with --trick-play
  master.m3u8         # HLS master (--hls)
  manifest.mpd        # DASH MPD (--dash)
```

With `--on-demand`, media is also concatenated into `rep_N.mp4` single files
referenced by `BaseURL` + byte ranges.

With `--format ts`, segments are `seg_N_M.ts` (no init / MAP).

With `--format packed-audio`, segments are raw `.aac` / `.ac3` elementary bursts.

### Flags — general packaging

| Flag | Default | Description |
|------|---------|-------------|
| `-o`, `--out <DIR>` | `out` | Output directory (created if missing) |
| `--segment-duration <SECS>` | `6` | Target segment length in seconds; cuts on keyframes |
| `--dash` | off | Write `manifest.mpd` |
| `--hls` | off | Write `master.m3u8` + per-track media playlists |
| `--format <FORMAT>` | `cmaf` | Segment container: `cmaf` \| `ts` \| `packed-audio` |
| `--on-demand` | off | DASH on-demand single-file (`isoff-on-demand` + `SegmentList` ranges) |
| `--parallel` | off | Package tracks concurrently (`std::thread::scope`) |
| `--http-push <URL>` | — | After local write, HTTP/1.1 `PUT` each object to `{URL}/{name}` (plain HTTP only) |

> **Note:** `--dash` and `--hls` are independent opt-in flags. Pass at least one
> (or both) or you only get segment files with no manifests.

### Flags — presentation (Phase 4)

| Flag | Default | Description |
|------|---------|-------------|
| `--presentation <MODE>` | `vod` | `vod` — static MPD, `#EXT-X-PLAYLIST-TYPE:VOD` + `#EXT-X-ENDLIST` |
| | | `event` — dynamic MPD, `#EXT-X-PLAYLIST-TYPE:EVENT` (no ENDLIST) |
| | | `live` — dynamic MPD, sliding window, no PLAYLIST-TYPE, no ENDLIST |
| `--live-window <N>` | live: `3` | Keep last *N* segments in live/event manifests |
| `--multi-period` | off | Each input file → successive DASH `Period` (not ABR ladder) |
| `--trick-play` | off | Keyframe-only tracks + DASH trick AdaptationSet + HLS I-frame playlists |
| `--low-latency` | off | LL-HLS `EXT-X-PART` / server control + LL-DASH `availabilityTimeOffset` |
| `--part-duration <SECS>` | `1` | Part target duration when `--low-latency` is set |
| `--availability-start-time <ISO8601>` | now | Wall-clock origin for live/event (`2026-07-18T00:00:00Z`) |
| `--scte35 <SPEC>` | — | Repeatable SCTE-35 marker (see [SCTE-35](#scte-35-markers)) |

#### Presentation modes

| Mode | DASH | HLS |
|------|------|-----|
| `vod` | `type="static"`, `mediaPresentationDuration` | `PLAYLIST-TYPE:VOD` + `ENDLIST` |
| `event` | `type="dynamic"`, live attributes | `PLAYLIST-TYPE:EVENT`, no `ENDLIST` until you re-package as VOD |
| `live` | `type="dynamic"`, sliding window, `UTCTiming` | No type tag, `#EXT-X-MEDIA-SEQUENCE`, sliding window |

Live packaging is **VOD-as-live**: a finished file is described with live
manifest tags. A continuously rewriting origin is available via
[`sheathe origin`](#sheathe-origin).

#### SCTE-35 markers

```text
--scte35 TIME[:out|in][:BREAK_DUR]
```

| Form | Meaning |
|------|---------|
| `--scte35 30` | Cue-out at 30s |
| `--scte35 30:out` | Cue-out at 30s |
| `--scte35 30:out:15` | Cue-out at 30s, planned break 15s |
| `--scte35 45:in` | Cue-in (return to network) at 45s |

Emits:

- DASH: `EventStream` with `schemeIdUri="urn:scte:scte35:2014:xml+bin"` (base64 splice)
- HLS: `#EXT-X-DATERANGE` with `SCTE35-OUT` / `SCTE35-IN` (hex `0x…`)

### Flags — encryption (Phase 2)

| Flag | Default | Description |
|------|---------|-------------|
| `--enc-key <KID:KEY>` | — | Raw key: 32 hex chars KID + `:` + 32 hex chars KEY |
| `--enc-key-file <PATH>` | — | Same `KID:KEY` from a file (comments with `#` allowed). Overrides `--enc-key` |
| `--enc-scheme <SCHEME>` | `cenc` | `cenc` (CTR) \| `cens` (CTR pattern) \| `cbc1` (CBC) \| `cbcs` (CBC pattern) |
| `--enc-key-uri <URI>` | `key.bin` | URI written into HLS `#EXT-X-KEY` |
| `--protection-systems <LIST>` | `common` | Comma list: `common`, `widevine`, `playready` |
| `--crypto-period-duration <SECS>` | — | Key rotation period; per-period keys + `seig` + moof `pssh` |

Example key file:

```text
# production asset key — do not commit
00112233445566778899aabbccddeeff:000102030405060708090a0b0c0d0e0f
```

```sh
sheathe package in.mp4 -o out/ --dash --hls \
  --enc-key-file keys/asset.key \
  --enc-scheme cbcs \
  --protection-systems widevine,playready \
  --crypto-period-duration 120
```

### Flags — segment format

| `--format` | Segments | Init / MAP | Typical use |
|------------|----------|------------|-------------|
| `cmaf` (default) | `.m4s` + `init_*.mp4` | Yes (`#EXT-X-MAP`) | DASH + fMP4 HLS |
| `ts` | `.ts` | No | Classic HLS TS |
| `packed-audio` | `.aac` / `.ac3` | No | Audio-only HLS |

`--on-demand` only applies to CMAF + DASH (single-file `rep_N.mp4`).

### Exit behaviour

- Non-zero on read/parse/write errors or invalid flag combinations (e.g.
  non-positive crypto period).
- Prints a short summary: rendition count, duration, segment count, paths
  written.

---

## `sheathe probe`

Inspect an input and print the streams sheathe would package.

```text
sheathe probe [OPTIONS] <INPUT>
```

### Examples

```sh
sheathe probe movie.mp4
sheathe probe broadcast.ts
sheathe probe clip.webm
sheathe probe stream.h264
sheathe probe captions.vtt
```

### Sample output

```text
probe: movie.mp4  (12345678 bytes, 2 track(s), MP4)
  [0] track #1  audio mp4a.40.2 48000Hz ~128kbps
       samples=12000  timescale=48000
  [1] track #2  video avc1.640028 1920x1080 ~4500kbps
       samples=7200  timescale=90000
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<INPUT>` | Path to a media file (same formats as `package`) |

No packaging is performed; useful for pipeline debugging and CI smoke checks.

---

## `sheathe origin`

Run a minimal pure-std HTTP/1.1 **JIT origin**: package media on demand and
serve manifests / cached segments.

```text
sheathe origin [OPTIONS]
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--bind <ADDR>` | `127.0.0.1:8787` | Listen address |
| `--media-root <DIR>` | `.` | Sandbox for relative `input=` paths |
| `--cache-dir <DIR>` | `/tmp/sheathe-origin` | Packaged output cache |
| `--segment-duration <SECS>` | `6` | Segment duration for JIT packages |

### HTTP API

| Method | Path | Response |
|--------|------|----------|
| `GET` | `/health` or `/healthz` | `200` plain `ok` |
| `GET` | `/package?input=<path>&format=hls\|dash\|both` | Master playlist or MPD (packages into cache if needed) |
| `GET` | `/out/<cache-rel>` | Cached object (segments, playlists) |

`input` may be relative to `--media-root` (path-traversal protected) or absolute
(when allowed by the sandbox rules).

### Examples

```sh
# Start origin
sheathe origin --bind 127.0.0.1:8787 --media-root ./corpus/media

# Package + fetch HLS master
curl -s 'http://127.0.0.1:8787/package?input=bear-1280x720.mp4&format=hls'

# DASH MPD
curl -s 'http://127.0.0.1:8787/package?input=bear-1280x720.mp4&format=dash'

# Health
curl -s http://127.0.0.1:8787/health
```

This is a **demo / development origin**, not a production CDN (no TLS, no auth).
For continuous live rewrite or multi-tenant origin behaviour, put a reverse
proxy / CDN in front and harden separately.

---

## Workflow recipes

### 1. Simple VOD site

```sh
sheathe package movie.mp4 -o public/vod/ \
  --dash --hls --segment-duration 4
# Serve public/vod/ over any static file server.
# DASH:  public/vod/manifest.mpd
# HLS:   public/vod/master.m3u8
```

### 2. Multi-bitrate ladder

```sh
sheathe package \
  encode/360p.mp4 encode/720p.mp4 encode/1080p.mp4 \
  -o ladder/ --dash --hls --segment-duration 6
```

Each input contributes its tracks as separate representations / variants.

### 3. Encrypted multi-DRM VOD

```sh
sheathe package movie.mp4 -o drm/ --dash --hls \
  --enc-key-file secrets/kid_key.txt \
  --enc-scheme cenc \
  --protection-systems common,widevine,playready \
  --enc-key-uri https://license.example/keys/asset.bin
```

### 4. Live window for a finished mezzanine

```sh
sheathe package mezz.mp4 -o live-sim/ --dash --hls \
  --presentation live \
  --live-window 5 \
  --segment-duration 2 \
  --availability-start-time 2026-07-18T12:00:00Z
```

### 5. Ad break markers

```sh
sheathe package episode.mp4 -o midroll/ --dash --hls \
  --scte35 600:out:30 \
  --scte35 630:in
```

### 6. Low-latency fMP4 HLS + DASH

```sh
sheathe package sports.mp4 -o ll/ --dash --hls \
  --presentation live \
  --low-latency --part-duration 0.5 \
  --live-window 4 \
  --segment-duration 2
```

### 7. Trick-play (I-frame) tracks

```sh
sheathe package film.mp4 -o scrub/ --dash --hls --trick-play
# HLS master includes #EXT-X-I-FRAME-STREAM-INF
# DASH includes trick-mode AdaptationSet + maxPlayoutRate
```

### 8. Classic MPEG-TS HLS

```sh
sheathe package movie.mp4 -o classic/ --hls --format ts --segment-duration 6
```

### 9. On-demand single-file DASH

```sh
sheathe package movie.mp4 -o od/ --dash --on-demand
# od/rep_*.mp4 + manifest.mpd with SegmentList mediaRange=
```

### 10. JIT package over HTTP

```sh
sheathe origin --bind 0.0.0.0:8787 --media-root /media --cache-dir /var/cache/sheathe
# Clients: GET /package?input=titles/a.mp4&format=hls
```

### 11. Push packaged objects to an ingest endpoint

```sh
sheathe package movie.mp4 -o /tmp/pkg --dash --hls \
  --http-push http://ingest.internal:9000/events/42
# Each file is PUT to http://ingest.internal:9000/events/42/<name>
```

### 12. Oracle check vs Shaka Packager

```sh
# Requires `packager` (Shaka) on PATH
just oracle movie.mp4
just oracle-corpus
```

### 13. Throughput micro-bench

```sh
just bench                              # default corpus bear, 5 runs
./scripts/bench_throughput.sh movie.mp4 10
```

---

## Environment & tooling

| Task | Command |
|------|---------|
| Build release | `cargo build -p sheathe --release` |
| Full CI gate | `just check-all` |
| Unit tests | `just test` |
| Clippy | `just lint` |
| Fetch corpus | `just corpus` |
| Shaka diff one file | `just oracle path/to/file` |
| Shaka diff corpus | `just oracle-corpus` |
| Throughput bench | `just bench` |
| Fuzz demuxers | `cargo +nightly fuzz run mp4_box_reader` (see [CONFORMANCE.md](./CONFORMANCE.md)) |

---

## Library API (related)

The CLI is a thin wrapper over crates:

| Crate | Role |
|-------|------|
| `sheathe-package` | `package()`, `probe()`, `serve_origin()`, IO sinks |
| `sheathe-dash` | MPD generation |
| `sheathe-hls` | Master / media playlists |
| `sheathe-mp4` | CMAF init + media segments |
| `sheathe-ts` | MPEG-TS demux + mux |
| `sheathe-crypto` | CENC schemes + `pssh` |

```rust
use sheathe_package::{PackageOptions, package};
use std::path::PathBuf;

let opts = PackageOptions {
    out_dir: PathBuf::from("out"),
    dash: true,
    hls: true,
    segment_duration: 6.0,
    ..Default::default()
};
let out = package(&[PathBuf::from("movie.mp4")], &opts)?;
```

See docs.rs for crate-level API docs after publish.

---

## Limitations (honest)

| Area | Status |
|------|--------|
| Live packaging | Finite files described as live; continuous window rewrite is JIT origin only |
| HTTPS push | Not in pure-std sink — terminate TLS externally or use HTTP |
| Origin | No TLS, no auth — demo/dev grade |
| Network DRM license servers | Deferred (need live Widevine/PlayReady endpoints) |
| Vorbis | Out of scope (no CMAF sample entry; Shaka rejects too) |
| `--dash` / `--hls` | Must be requested explicitly |

---

## See also

- [README.md](../README.md) — project overview
- [ROADMAP.md](../ROADMAP.md) — phase status
- [CONFORMANCE.md](./CONFORMANCE.md) — oracle & external validators
- [CONTRIBUTING.md](../CONTRIBUTING.md) — development workflow
