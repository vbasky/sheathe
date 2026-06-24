# Changelog

All notable changes to **sheathe** are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/); this project adheres to
[Semantic Versioning](https://semver.org/).

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

[0.1.1]: https://github.com/vbasky/sheathe/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vbasky/sheathe/releases/tag/v0.1.0
