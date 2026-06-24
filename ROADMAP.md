# ROADMAP — sheathe → Shaka Packager parity

**Goal:** a pure-Rust media packager at functional parity with
[Shaka Packager](https://github.com/shaka-project/shaka-packager) for the
features production OTT actually uses.

**Oracle / method (the `revelo method`):** every output is differential-tested
against Shaka Packager (and, where useful, validated by independent tools —
ffmpeg decode/decrypt, Apple `mediastreamvalidator`, DASH-IF conformance). We do
not claim a feature "done" until its bytes are verified against the oracle.

Legend: ✅ done · 🟡 in progress · ⬜ planned.

---

## Phase 0 — Foundations ✅

- ✅ Cargo workspace + crate split (`core`, `mp4`, `dash`, `hls`, `crypto`, `cli`).
- ✅ Core media model: `StreamInfo`, `Sample`, `SampleFlags`, `Timescale`, `Error`.
- ✅ ISO-BMFF `BoxWriter` (nestable, backpatching) and dependency-free box reader + `Cursor`.
- ✅ `Fragmenter` — keyframe-aligned, target-duration segmentation.
- ✅ CI MSRV guard (reads `rust-version` from `Cargo.toml`, `cargo check --workspace`).

## Phase 1 — VOD packaging (MP4 in → CMAF + DASH/HLS out) ✅

- ✅ **MP4 demuxer**: box traversal + full sample-table reconstruction
  (`stts`/`ctts`/`stsc`/`stsz`/`stz2`/`stco`/`co64`/`stss`); per-sample
  dts/pts/duration/keyframe + data; average bitrate.
- ✅ **CMAF writer**: init (`ftyp`+`moov`+`mvex`/`trex`) and media
  (`styp`+`sidx`+`moof`+`mdat`) segments; ffmpeg decode-verified.
- ✅ **DASH** static MPD (`SegmentTemplate` + `SegmentTimeline`, live profile).
- ✅ **HLS** master + VOD media playlists (`EXT-X-MAP` + per-segment `EXTINF`).
- ✅ **RFC 6381 codec strings** (`avc1.*`, `mp4a.40.*`, `hvc1.*`) from `avcC`/`esds`/`hvcC`.
- ✅ **CENC `cenc` encryption** (AES-128-CTR): AES core (NIST-vector tested);
  NAL-aware subsample encryption (AVC/HEVC) + full-sample audio; `encv`/`enca` +
  `sinf`/`frma`/`schm`/`tenc`; common `pssh`; per-segment `senc`; raw-key via
  `--enc-key KID:KEY`. **ffmpeg decrypt+decode verified (video 360 frames + audio).**
- ✅ **HLS `EXT-X-MEDIA` audio rendition groups**: audio as a group, video
  `EXT-X-STREAM-INF` references it with combined `CODECS`/`BANDWIDTH` and `AUDIO=`.
- ✅ **Multi-input ABR ladder**: `package in_a.mp4 in_b.mp4 …` → one DASH
  AdaptationSet / HLS master with a Representation/variant per input rendition.
- ✅ **`av01` (AV1) codec string** from `av1C` (e.g. `av01.0.00M.08`); ffprobe-verified.

(DASH on-demand `SegmentBase`/byte-range single-file output → Phase 5;
WebVTT/TTML text passthrough → Phase 3, alongside the other input/codec work.)

## Phase 2 — Encryption & DRM 🟡

`cenc` shipped as part of Phase 1 (VOD core). Remaining DRM breadth:

- 🟡 **CENC `cbcs` (AES-128-CBC, pattern 1:9)** — AES-CBC pattern core done +
  NIST-vector tested; writer wiring drafted (constant-IV `tenc` v1, pattern
  `senc`, `cbcs` `schm`) but **not yet ffmpeg-verified**.
- ⬜ `saiz`/`saio` aux-info boxes (conformance; ffmpeg already decrypts via `senc`).
- ⬜ `cbc1` / `cens` schemes.
- ⬜ Key sources beyond raw key: Widevine key server, PlayReady, key files.
- ⬜ Multi-DRM `pssh` (Widevine + PlayReady + common) and key rotation.
- ⬜ HLS `SAMPLE-AES` + `EXT-X-KEY` / `EXT-X-SESSION-KEY` signalling.

## Phase 3 — Inputs & codecs ⬜

- ⬜ MPEG-2 TS demux (PAT/PMT/PES, ADTS-AAC, H.264/H.265 in Annex B).
- ⬜ WebM/Matroska demux (VP8/VP9/AV1/Opus/Vorbis).
- ⬜ Raw elementary stream inputs (H.264/H.265 Annex B, AAC-ADTS, AC-3).
- ⬜ Audio: AC-3 / E-AC-3, Opus, FLAC, MP3 sample entries + codec strings.
- ⬜ Text passthrough: WebVTT / TTML (IMSC) segmented output.
- ⬜ Caption extraction: CEA-608/708 from SEI → segmented WebVTT/TTML.

## Phase 4 — Live & advanced manifests ⬜

- ⬜ Dynamic DASH (`type="dynamic"`, availability/UTCTiming, rolling timeline).
- ⬜ Live HLS (sliding window, `EXT-X-MEDIA-SEQUENCE`, `EVENT`/`VOD` types).
- ⬜ Multi-period DASH; period continuity.
- ⬜ Trick-play (DASH trick-mode AdaptationSet; HLS I-frame playlists).
- ⬜ Low latency: LL-HLS (partial segments, preload hints) and LL-DASH (chunked `moof`).
- ⬜ SCTE-35 ad markers → DASH `EventStream` / HLS `EXT-X-DATERANGE`.

## Phase 5 — Output formats, IO, operations ⬜

- ⬜ DASH on-demand profile with `SegmentBase` + byte-range (single-file output).
- ⬜ Packed-audio HLS output (raw AAC/AC-3) and TS output muxer.
- ⬜ IO backends: file, HTTP(S) push, UDP/live ingest.
- ⬜ JIT / origin mode (package on request).
- ⬜ Pipeline parallelism + throughput benchmarks vs Shaka.

## Cross-cutting — Conformance & quality ⬜ / 🟡

- 🟡 Hermetic unit/integration tests per crate (synthetic MP4, structural assertions).
- ⬜ **Differential harness vs Shaka Packager**: run both on a corpus; diff CMAF
  box structure, MPD (canonical XML), and HLS (normalized) — track oracle deltas.
- ⬜ External conformance: DASH-IF validator, Apple `mediastreamvalidator`, Widevine/PlayReady test vectors.
- ⬜ Fuzzing of the demuxer/box reader.

---

## Current focus

**Phase 1 is complete.** MP4 in → CMAF out (clear or `cenc`-encrypted), DASH +
HLS with correct codec strings, audio rendition groups, and multi-input ABR.
Next up is **Phase 2** (resume `cbcs` and broader DRM) or **Phase 3** (more
inputs/codecs — MPEG-TS, WebM, additional audio) — to be picked next.
