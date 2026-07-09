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

## Phase 3 — Inputs & codecs ✅

- ✅ MPEG-2 TS demux (PAT/PMT/PES, ADTS-AAC, H.264/H.265 in Annex B).
  `sheathe-ts`: PAT/PMT/PES reassembly, H.264 + HEVC Annex B + ADTS-AAC sample
  extraction, `avc1`/`hvc1`/`mp4a` sample-entry synthesis; wired into `probe` and
  `package`. Hermetic synthetic-TS tests **plus a real-corpus oracle**
  (`just oracle-corpus`): decode + Shaka cross-check, and CEA-608 vs ccextractor.
- ✅ Fragmented-MP4 (CMAF) input. `sheathe-mp4` reconstructs samples from
  `moof`/`traf`/`trun` (+ `trex`/`tfdt` defaults) when the `moov` sample table is
  empty; real AV1/AC-3/E-AC-3 fMP4 assets decode-verified against the corpus.
- ✅ WebM/Matroska demux (VP8/VP9/AV1/Opus/Vorbis). `sheathe-mkv`: EBML reader,
  Segment/Info/Tracks/Cluster + SimpleBlock/BlockGroup extraction; VP8/VP9/AV1
  video + Opus audio with `vp08`/`vp09`/`av01`/`Opus` sample-entry synthesis;
  wired into `probe`/`package`. **VP8/VP9/AV1/Opus decode-verified against a real
  WebM corpus; VP9 and Opus codec strings byte-match Shaka Packager**
  (`vp09.00.20.08.01.02.02.02.00`, `opus`). **All four lacing modes implemented**
  (none, Xiph, fixed-size, EBML) — each splits block payloads into individual
  frames per the Matroska spec, with hermetic unit tests. **Vorbis is out of scope
  for CMAF output** — there is no standard ISO-BMFF Vorbis sample entry and Shaka
  Packager itself rejects it (`NOTIMPLEMENTED`); it stays a WebM/Ogg-only codec.
- ✅ Raw elementary stream inputs (H.264/H.265 Annex B, AAC-ADTS, AC-3).
  `sheathe-es`: extension + content sniffing, Annex B access-unit splitting,
  ADTS-AAC and AC-3 syncframe extraction; wired into `probe` and `package`.
  **H.264 / AAC / FLAC / AC-3 raw ES decode-verified against the corpus** —
  plus CEA-708 captions from `bear-708.h264` (150/150 cues decoded).
- ✅ Audio: **AC-3** (`ac-3`/`dac3`), **E-AC-3** (`ec-3`/`dec3`), **MP3**
  (`mp4a` OTI `0x6B`/`0x69`), and **FLAC** (`fLaC`/`dfLa`) — parsers +
  sample-entry synthesis + codec strings. **Opus** (`Opus`/`dOps`, from
  `OpusHead`) via `sheathe-mkv`. All verified against the real corpus (decode +
  Shaka cross-check for AC-3/E-AC-3/FLAC).
- ✅ Text passthrough: WebVTT / TTML (IMSC) segmented output. `sheathe-text`:
  **WebVTT done** — `.vtt` → gapless ISO 14496-30 `wvtt` samples (`vttc`/`sttg`/
  `payl`/`vtte`) + `wvtt`/`vttC` entry, wired into `probe`/`package` with a DASH
  text AdaptationSet. **TTML/IMSC** (`stpp`) — passthrough parser (`<tt` root
  detection, SMPTE duration) + `stpp` sample entry + `Codec::Stpp` variant;
  wired into CLI, probe- and package-verified.
- ✅ Caption extraction: CEA-608/708 from SEI → segmented WebVTT/TTML.
  **CEA-608** (field 1 + field 2, pop-on/roll-up) and **CEA-708** (DTVCC packet
  reassembly, service blocks, C0/C1/G0/G1 + 8-window model) both decode `GA94`
  SEI `cc_data` to WebVTT, auto-appended as one `wvtt` track per source in
  `probe`/`package`. **CEA-608 verified against ccextractor on a real corpus
  (Apple BipBop): 73/73 cue words and all 19 `line:` positions match** — pop-on
  captions are row-addressed (PAC → WebVTT `line:%`), so vertical positioning now
  renders. **CEA-708 decoder verified against a synthetic SEI-embedded corpus
  asset** (`bear-708.h264`, 150 frames / 150 captions decoded correctly — the
  corpus manifest includes it). **Pen colour now rendered** — SPA/SPC/SWA
  command parsing captures 6-bit RGB foreground/background colours (64-colour
  palette) plus italic/underline; the decoder emits a `STYLE` block with
  `::cue(.fg_r_g_b) { color: rgb(...) }` when colours are present, backward
  compatible when absent.

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
- ✅ **Differential harness vs Shaka Packager**: `just oracle <input>` runs both;
  diff CMAF box structure, MPD (canonical XML), and HLS (normalized).
- ✅ **Real-media oracle corpus** (`corpus/` + `just oracle-corpus`): a
  fetch-on-demand, checksum-pinned corpus (Chromium media test data, Apple BipBop)
  mapped to per-asset oracles — decode (ffprobe), Shaka cross-check, and CEA-608
  vs ccextractor. 16/17 assets green; the one XFAIL is Ogg/Vorbis (open). This is
  the regression gate that promoted the Phase 3 items above from 🟡 to verified.
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

**Current focus: Phase 3 done.** The inputs & codecs phase is complete across
all seven crates — MPEG-TS, fragmented-MP4, WebM/Matroska, raw elementary
streams, the full audio codec set (AC-3, E-AC-3, MP3, FLAC, Opus), WebVTT and
TTML/IMSC text passthrough, CEA-608 and CEA-708 caption extraction with colour
styling, and all four WebM lacing modes. 17/17 corpus assets green (the one
XFAIL is Ogg/Vorbis, explicitly out of scope). **Phase 4 (Live & Advanced
Manifests) and Phase 5 (Output Formats, IO, Operations) are the next frontier
— and where Shaka Packager's remaining surface area lives.**

Remaining before **0.3** (no partial milestone releases): **nothing in Phase 3**
— all items are verified. **Vorbis** is explicitly out of scope — CMAF has no
standard Vorbis sample entry and Shaka Packager rejects it too.
