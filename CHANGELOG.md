# Changelog

All notable changes to **sheathe** are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/); this project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.7.0] — 2026-08-22

### Added

- **Stream descriptors** — Shaka-style `in=file,stream=audio,language=eng,…`
  positional args (bare paths still mean every track). Per-stream bandwidth,
  language, playlist names, HLS NAME/GROUP-ID/CHARACTERISTICS, DASH roles /
  accessibilities, `skip_encryption`, `drm_label`, `trick_play_factor`,
  `forced_subtitle`.
- **HLS/DASH knobs** — `--hls-base-url`, `--hls-media-sequence-number`,
  `--hls-start-time-offset`, `--create-session-keys`, `--add-program-date-time`,
  `--closed-captions`, `--use-legacy-vp9-codec-string`,
  `--dash-add-last-segment-number`, `--segment-template-constant-duration`,
  `--use-dovi-supplemental-codecs`, `--mvex-before-trak`.
- **DRM extras** — `--enc-scheme aes128` (HLS AES-128 full-segment),
  `--crypt-byte-block` / `--skip-byte-block`, `--playready-extra-header-data`,
  `--keys` labeled raw keys, `--decrypt` of CENC inputs, `--cpix` / SPEKE,
  Widevine key-server client (`--enable-widevine-encryption`,
  `--key-server-url`, AES/RSA request signing).
- **Live / IO** — HTTPS push (rustls), `--ignore-http-output-failures`,
  `--user-agent`, `--ca-file`, client certs, `--live-rewrite`. Origin
  `--tls-cert` / `--tls-key` wrap the accept loop in rustls (HTTPS); `--auth
  user:pass` is HTTP Basic.

### Changed

- **CLI `--help`** is now Shaka Packager-style: grouped headings (Output /
  Presentation / Encryption), long flag descriptions, examples, and a
  `packager` flag map. `-h` stays compact.
- **`sheathe probe`** dumps stream info in a `--dump_stream_info` layout
  (type, codec, codec_string, time_scale, duration, samples, resolution /
  sample rate, bitrate) instead of a two-line summary.
- **`sheathe package`** prints a fuller run summary (inputs, format,
  presentation, encryption, init paths) and ends with
  `Packaging completed successfully.`

### Fixed

- **CLI banner** no longer sends iTerm2 inline images to Warp (Failed image
  placeholder) or Ghostty (no OSC 1337). Ghostty/kitty/WezTerm/rio get the
  Kitty graphics protocol; iTerm/Konsole keep OSC 1337; everyone else gets
  the dagger wordmark.
- **`sheathe` with no command** prints help to stdout and exits 0, instead of
  treating a missing subcommand as a failure (stderr + exit 2).

## [0.6.0] — 2026-08-21

### Changed

- **`sheathe-cli` is the binary crate** (`[[bin]] name = "sheathe"`);
  `sheathe` is a library facade that re-exports `sheathe-package` and the
  format crates. Install the CLI with `cargo install sheathe-cli`.
  **Breaking:** `cargo install sheathe` no longer installs a binary
  (`sheathe` has no `[[bin]]`).
- **crates.io publish is part of the GitHub Release workflow.** Pushing a
  `v*` tag builds binaries, creates the GitHub Release, and publishes
  crates in dependency order (skipping versions already on crates.io).

## [0.5.0] — 2026-07-18

### Added

- **CLI documentation** — full command reference at
  [`docs/CLI.md`](docs/CLI.md) (every subcommand, flag, output layout, and
  workflow recipe). README usage section expanded with a command table and
  quick-start gallery.

- **Phase 5 — Output formats, IO, operations**
  - **DASH on-demand** (`--on-demand`): single-file `rep_N.mp4` with
    `isoff-on-demand` profile, `BaseURL` + `SegmentList` byte ranges.
  - **MPEG-TS mux** (`--format ts`): pure-Rust PAT/PMT/PES muxer in
    `sheathe-ts::mux`; HLS media playlists without `#EXT-X-MAP`.
  - **Packed-audio HLS** (`--format packed-audio`): elementary ADTS/AC-3
    segments for audio-only delivery.
  - **IO backends**: `FileSink`, pure-std `HttpPushSink` (`--http-push URL`),
    `udp://host:port` ingest via `read_input`.
  - **JIT origin**: `sheathe origin --bind 127.0.0.1:8787` packages on
    `GET /package?input=…&format=hls|dash`.
  - **Parallel packaging** (`--parallel`) and `just bench` throughput script
    vs Shaka Packager.
  - **Fuzz targets** under `fuzz/` + conformance guide
    (`docs/CONFORMANCE.md`).

- **Phase 4 — Live & advanced manifests**
  - **Dynamic DASH / live HLS**: `--presentation vod|event|live` with
    `availabilityStartTime`, `publishTime`, `minimumUpdatePeriod`,
    `timeShiftBufferDepth`, `suggestedPresentationDelay`, `UTCTiming`, and
    HLS sliding windows (`--live-window N`, `#EXT-X-MEDIA-SEQUENCE`, no
    `#EXT-X-ENDLIST` for live/open events).
  - **Multi-period DASH** (`--multi-period`): each input is a successive
    `Period` with continuous `@start` offsets (instead of a single-period ABR
    ladder).
  - **Trick-play** (`--trick-play`): keyframe-only CMAF segments, DASH
    trick-mode AdaptationSet (`EssentialProperty` + `maxPlayoutRate`), HLS
    `#EXT-X-I-FRAME-STREAM-INF` / `#EXT-X-I-FRAMES-ONLY` playlists.
  - **Low latency** (`--low-latency --part-duration S`): LL-HLS
    (`EXT-X-PART` / `PART-INF` / `SERVER-CONTROL` / `PRELOAD-HINT`) and
    LL-DASH (`availabilityTimeOffset`, `availabilityTimeComplete="false"`)
    with sample-budgeted part segments.
  - **SCTE-35 markers** (`--scte35 TIME[:out|in][:BREAK_DUR]`): pure-Rust
    `splice_insert` builder (MPEG CRC-32) → DASH `EventStream`
    (`urn:scte:scte35:2014:xml+bin`, base64) and HLS `#EXT-X-DATERANGE`
    (`SCTE35-OUT` / `SCTE35-IN`, hex).
  - Library surface: expanded `sheathe-dash::Manifest` (periods, live fields,
    events) and `sheathe-hls::MediaPlaylist` (VOD/EVENT/live, parts,
    dateranges, I-frame helper); `sheathe-package::{PresentationMode,
    Scte35Marker, PackageOptions}` Phase 4 fields.

Phases 0–5 of the Shaka Packager parity roadmap are complete. Deferred (not
blocking): live DRM license-server clients, production TLS origin/HTTPS push,
Ogg/Vorbis (out of scope).

## [0.3.0] — 2026-07-09

### Added

- **WebM all-lacing support** (Xiph + EBML lacing): both missing lacing modes
  implemented in `sheathe-mkv` — `split_xiph_lacing` reads Xiph-style varints
  (0xFF=+255 continuation), `split_ebml_lacing` reads first-frame size as
  unsigned EBML varint + subsequent sizes as signed EBML varints with the
  spec-correct `signed = unsigned - 2^(7n-1)` formula. Hermetic unit tests
  cover all lacing modes (none, fixed, Xiph, EBML) including edge cases.
- **TTML/IMSC (stpp) text passthrough**: `sheathe-text` now provides
  `pub fn ttml(text: &str) -> Result<TextTrack>` — detects `<tt` root (skipping
  optional XML declaration), extracts SMPTE `dur="HH:MM:SS.mmm"`, produces an
  `stpp` sample entry (`Codec::Stpp` variant) with the TTML document as a
  single `payl` sample. Wired into CLI `probe`/`package`.
- **CEA-708 caption colour styling**: SPA (SetPenAttributes), SPC (SetPenColor),
  and SWA (SetWindowAttributes) commands are now parsed — 6-bit RGB
  foreground/background colours (64-colour palette) plus italic/underline.
  The decoder emits a WebVTT `STYLE` block with `::cue(.fg_r_g_b) {
  color: rgb(...) }` when colour info is present; fully backward compatible
  when absent.
- **CEA-708 real-corpus asset**: `corpus/media/bear-708.h264` — a synthetic
  H.264 Annex B file (150 frames, 10 cycling captions) with CEA-708 DTVCC
  captions embedded in SEI. Generated by `cargo test -p sheathe-text --
  generate_708_corpus_asset --ignored`. Verified 150/150 captions decode
  correctly end-to-end.
- **fMP4 (fragmented MP4) input support**: `sheathe-mp4` reconstructs samples
  from `moof`/`traf`/`trun` boxes when the `moov` sample table is empty,
  enabling CMAF/segmented file demuxing.

### Fixed

- **Access-unit partitioning**: raw H.264 Annex B streams with per-frame SEI
  NALs are now correctly split — slice order (slice before SEI in each AU)
  prevents the partitioner from merging adjacent frames' non-VCL NALs.
- **CEA-608 field 1/2 parity handling**: correctly strips odd-parity from
  CEA-608 byte pairs, fixing decoding of real-world caption data.


## [0.2.0] — 2026-06-25

### Added

- **CENC `cbc1` and `cens` schemes** (`--enc-scheme cbc1|cens`), completing the
  ISO/IEC 23001-7 scheme matrix alongside `cenc`/`cbcs`. `cbc1` is full-region
  AES-128-CBC with a per-sample IV; `cens` is AES-128-CTR with 1:9 pattern
  encryption. `tenc` version, crypt/skip pattern bytes, and `senc` subsample
  signalling were differentially matched against Shaka Packager, and every
  scheme is ffmpeg decrypt+decode verified (video + audio frame md5).
- **Multi-DRM `pssh`** (`--protection-systems common,widevine,playready`):
  Widevine (`WidevinePsshData` protobuf), PlayReady (a `WRMHEADER` 4.0.0.0
  PlayReady Object with swapped-GUID KID and AES-ECB checksum), and Common (v1
  KID list), all generated from the raw key. Each box **byte-matches** Shaka
  Packager's `--protection_systems` output.
- **Key rotation** (`--crypto-period-duration <seconds>`): per-period content
  keys derived by left-rotating the base key (Shaka's naive raw-key scheme),
  signalled per segment via `seig` (`sbgp`/`sgpd`) sample groups, a zero-KID
  init `tenc`, and a per-period `pssh` in each `moof`. Box format matches Shaka;
  each segment decrypts to the clear baseline under its derived key.
- **Key-file input** (`--enc-key-file <path>`): reads the raw `KID:KEY` from a
  file (with `#` comments), keeping the key out of the process arguments.

### Changed

- **Pattern encryption is now video-only**, matching Shaka / DASH-IF: under
  `cens`/`cbcs`, audio is encrypted whole-sample (`tenc` pattern `0:0`) instead
  of carrying the 1:9 pattern. Audio tracks now emit `senc` without the
  `use_subsamples` flag or subsample entries (whole-sample encryption), which
  the patternless `cbcs`/`cens` audio case requires for player compatibility.

## [0.1.4] — 2026-06-24

### Added

- **`saiz`/`saio`** CENC sample-auxiliary-information boxes in media segments
  (DASH-IF conformance); `saio` offset is backpatched to point at the `senc`
  per-sample data (verified). ffmpeg decryption still passes.
- **HLS `#EXT-X-KEY`** encryption signalling in media playlists
  (`SAMPLE-AES-CTR` for `cenc`, `SAMPLE-AES` for `cbcs`) with a `--enc-key-uri`
  key-delivery URI — the HLS counterpart to DASH `ContentProtection`.

## [0.1.3] — 2026-06-24

### Added

- DASH `ContentProtection` signalling (`mp4protection` + `cenc:default_KID`) for
  encrypted output, so players recognise protected content.

### Changed

- **Conformed the workspace to the `rust-boilerplate` template**: edition 2024,
  `resolver = "3"`, `[workspace.lints]` (rust / rustdoc / clippy) inherited by
  every crate, `thiserror` 2.
- **READMEs** now symlink the root `README.md` in every crate (so each renders on
  crates.io); the root README gained badges and an absolute banner URL, and the
  banner moved to `docs/`.
- Added template tooling: `rustfmt.toml`, `rust-toolchain.toml`, `deny.toml`,
  `justfile`, `.editorconfig`, `.githooks/pre-commit`, `CONTRIBUTING.md`,
  `scripts/release.sh`, Dependabot, and a PR template. CI now also runs the doc
  build and `cargo-deny`.

## [0.1.2] — 2026-06-24

### Added

- **CENC `cbcs`** (AES-128-CBC, pattern 1:9) end-to-end — constant-IV `tenc` v1,
  pattern `senc`, `cbcs` `schm`; `--enc-scheme cbcs`. ffmpeg decrypt+decode verified.
- **DASH `ContentProtection`** signalling for encrypted output
  (`mp4protection` scheme + `cenc:default_KID`).
- **Per-crate READMEs** so every crate renders documentation on crates.io.

### Changed

- MSRV recorded as **1.85** in the published crate metadata (the dependency tree
  requires it); all crates republished at 0.1.2.

## [0.1.1] — 2026-06-24

### Added

- **`sheathe` crate** — the canonical install target: `cargo install sheathe`
  provides the `sheathe` binary.
- **Release workflow** (`.github/workflows/release.yml`): on a `v*` tag, builds
  the `sheathe` binary for `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`,
  and `x86_64-apple-darwin`, and attaches the archives to a GitHub Release.

### Changed

- **`sheathe-cli` is now library-only**, exposing `pub fn run()`; the binary it
  used to provide moved to the new `sheathe` crate (resolves the duplicate
  `sheathe` binary-name collision).
- **MSRV raised to 1.85.** The dependency tree (`clap_lex` 1.1.0) is edition
  2024, whose Cargo feature stabilized in Rust 1.85; the CI MSRV guard caught
  that 1.82 no longer builds. CI now passes.

## [0.1.0] — 2026-06-24

Initial release — a pure-Rust HLS/DASH/CMAF media packager, validated against
Shaka Packager as the reference oracle.

### Added

- Cargo workspace: `sheathe-core`, `sheathe-mp4`, `sheathe-dash`, `sheathe-hls`,
  `sheathe-crypto`, `sheathe-cli`.
- **MP4 demuxer**: box reader + full sample-table reconstruction
  (`stts`/`ctts`/`stsc`/`stsz`/`stz2`/`stco`/`co64`/`stss`).
- **CMAF writer**: init (`ftyp`+`moov`+`mvex`) and media
  (`styp`+`sidx`+`moof`+`mdat`) segments; ffmpeg decode-verified.
- **DASH** (`SegmentTemplate` + `SegmentTimeline`) and **HLS** (master + media
  playlists, `EXT-X-MAP`, `EXT-X-MEDIA` audio rendition groups).
- **RFC 6381 codec strings** from `avcC`/`hvcC`/`av1C`/`esds`
  (`avc1`/`hvc1`/`av01`/`mp4a`).
- **CENC `cenc` encryption** (AES-128-CTR): NAL-aware subsamples,
  `encv`/`enca`+`sinf`/`tenc`, `pssh`, `senc`; raw-key CLI; ffmpeg
  decrypt+decode verified.
- **Multi-input ABR ladder**: several inputs → one DASH/HLS manifest.
- CI with an MSRV guard that reads `rust-version` from `Cargo.toml`.

[0.6.0]: https://github.com/vbasky/sheathe/compare/v0.5.0...v0.6.0
[0.2.0]: https://github.com/vbasky/sheathe/compare/v0.1.4...v0.2.0
[0.1.4]: https://github.com/vbasky/sheathe/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/vbasky/sheathe/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/vbasky/sheathe/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vbasky/sheathe/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vbasky/sheathe/releases/tag/v0.1.0
