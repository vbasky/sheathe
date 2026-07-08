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

## Phase 2 — Encryption & DRM ✅

`cenc` shipped as part of Phase 1 (VOD core); the rest of the DRM breadth — the
full CENC scheme matrix, multi-DRM `pssh`, key rotation, and key-file input — is
now complete and oracle-verified. The only deferred item is live network key
servers (an external-service dependency, see below).

- ✅ **CENC `cbcs` (AES-128-CBC, pattern 1:9)**: AES-CBC pattern core (NIST-tested);
  constant-IV `tenc` v1, pattern `senc`, `cbcs` `schm`. **ffmpeg decrypt+decode
  verified (video 360 frames + audio).**
- ✅ **Manifest encryption signalling**: DASH `ContentProtection`
  (`mp4protection` + `cenc:default_KID`); `saiz`/`saio` aux-info boxes (offset
  backpatched to the `senc` data, verified); HLS `#EXT-X-KEY`
  (`SAMPLE-AES` / `SAMPLE-AES-CTR`) with key-delivery URI.
- ✅ **`cbc1` / `cens` schemes** — completing the full CENC scheme matrix.
  `cbc1` = AES-128-CBC full-region (per-sample IV, block-aligned subsamples);
  `cens` = AES-128-CTR with 1:9 pattern. Pattern encryption applied to video
  only; audio is whole-sample (no subsamples) under all schemes, per Shaka /
  DASH-IF. **`tenc`/`senc` structurally diffed against Shaka Packager; all four
  schemes ffmpeg decrypt+decode verified (video + audio frame md5).**
- ✅ **Multi-DRM `pssh`** (Widevine + PlayReady + Common) via `--protection-systems`.
  Widevine protobuf, PlayReady `WRMHEADER` 4.0.0.0 (swapped-GUID KID + AES-ECB
  checksum), Common v1 KID list — all generated from the raw key. **Each box
  byte-matches Shaka Packager's `--protection_systems` output.**
- ✅ **Key rotation** (`--crypto-period-duration`): per-period keys derived by
  left-rotating the base key (Shaka's naive raw-key scheme), signalled per
  segment with `seig` (`sbgp`/`sgpd`) sample groups + a zero-KID init `tenc` +
  per-period `pssh` in each `moof`. Box format matches Shaka; every segment
  decrypts to the clear baseline under its derived key. (sheathe maps periods
  straight from segment time, `floor(t/period)`, rather than replicating Shaka's
  one-segment prefetch lag.)
- ✅ **Key file source** (`--enc-key-file`) — raw key from a file, keeping it out
  of the process arguments.
- ⬜ Network key servers (Widevine / PlayReady): require client certificates and
  a live server endpoint, so they can't be implemented or oracle-verified in
  this hermetic setup — deferred as an external-service dependency.

## Phase 3 — Inputs & codecs 🟡

- ✅ MPEG-2 TS demux (PAT/PMT/PES, ADTS-AAC, H.264/H.265 in Annex B).
  `sheathe-ts`: PAT/PMT/PES reassembly, H.264 + HEVC Annex B + ADTS-AAC sample
  extraction, `avc1`/`hvc1`/`mp4a` sample-entry synthesis; wired into `probe` and
  `package`. Hermetic synthetic-TS integration tests. Oracle diff on a real TS
  corpus still open.
- 🟡 WebM/Matroska demux (VP8/VP9/AV1/Opus/Vorbis). `sheathe-mkv`: EBML reader,
  Segment/Info/Tracks/Cluster + SimpleBlock/BlockGroup extraction; VP8/VP9/AV1
  video + Opus audio with `vp08`/`vp09`/`av01`/`Opus` sample-entry synthesis;
  wired into `probe`/`package`. Vorbis, Xiph/EBML lacing, and bitstream-accurate
  `vpcC`/`av01` codec strings still open; oracle diff on a real corpus open.
- 🟡 Raw elementary stream inputs (H.264/H.265 Annex B, AAC-ADTS, AC-3).
  `sheathe-es`: extension + content sniffing, Annex B access-unit splitting,
  ADTS-AAC and AC-3 syncframe extraction; wired into `probe` and `package`.
  Oracle diff on a real corpus still open.
- 🟡 Audio: **AC-3** (`ac-3`/`dac3`), **E-AC-3** (`ec-3`/`dec3`), **MP3**
  (`mp4a` OTI `0x6B`/`0x69`), and **FLAC** (`fLaC`/`dfLa`) done — parsers +
  sample-entry synthesis + codec strings, all ffprobe-verified. **Opus**
  (`Opus`/`dOps`, built from `OpusHead`) done via `sheathe-mkv`, ffprobe-verified.
- 🟡 Text passthrough: WebVTT / TTML (IMSC) segmented output. `sheathe-text`:
  **WebVTT** done — `.vtt` → gapless ISO 14496-30 `wvtt` samples (`vttc`/`sttg`/
  `payl`/`vtte`) + `wvtt`/`vttC` entry, wired into `probe`/`package` with a DASH
  text AdaptationSet. **TTML/IMSC** (`stpp`) still open.
- 🟡 Caption extraction: CEA-608/708 from SEI → segmented WebVTT/TTML.
  **CEA-608** (field 1 + field 2, pop-on/roll-up) and **CEA-708** (DTVCC packet
  reassembly, service blocks, C0/C1/G0/G1 + 8-window model) both decode `GA94`
  SEI `cc_data` to WebVTT, auto-appended as one `wvtt` track per source in
  `probe`/`package`. Remaining: pen/window *styling* (positioning, colour) is
  length-skipped not rendered; real-corpus oracle diff still open.

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

- 🟡 Hermetic unit/integration tests per crate (synthetic MP4 + MPEG-TS, structural assertions).
- 🟡 **Differential harness vs Shaka Packager**: `just oracle <input>` runs both;
  diff CMAF box structure, MPD (canonical XML), and HLS (normalized) — track oracle deltas.
- ⬜ External conformance: DASH-IF validator, Apple `mediastreamvalidator`, Widevine/PlayReady test vectors.
- ⬜ Fuzzing of the demuxer/box reader.

---

## Current focus

**Phases 1 and 2 are complete.** The CENC scheme matrix (`cenc`/`cbcs`/`cbc1`/
`cens`), multi-DRM `pssh` (Widevine + PlayReady + Common), key rotation, and
key-file input all ship end-to-end, structurally diffed against Shaka Packager
and ffmpeg decrypt+decode verified. The only Phase 2 item left is live network
key servers, deferred as an external-service dependency that can't be
oracle-verified in a hermetic setup.

**Current focus: Phase 3** — the inputs & codecs phase is functionally landed
across seven crates. Shipped this cycle: MPEG-TS demux (`sheathe-ts`), raw
elementary streams (`sheathe-es`), the full audio set — **AC-3, E-AC-3, MP3,
FLAC, Opus** — plus the **WebM/Matroska** demuxer (`sheathe-mkv`, VP8/VP9/AV1 +
Opus), **WebVTT** text (`sheathe-text`), and **CEA-608** caption extraction.
Every codec is ffprobe-verified through `probe`/`package`.

Captions now cover **CEA-608 (fields 1+2) and CEA-708 (DTVCC)**, decoded to
`wvtt` tracks. Remaining before **0.3** (no partial milestone releases): a
real-corpus oracle diff vs Shaka Packager across the new inputs; the last
codec/format gaps — Vorbis in WebM, bitstream-accurate `vpcC`/`av01` codec
strings, WebM lacing variants, **TTML/IMSC** (`stpp`) text, and caption
*styling* (708 pen/window colour & positioning). The Shaka oracle harness
(`just oracle`) is scaffolded for corpus regression as inputs broaden.
