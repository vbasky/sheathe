# Changelog

All notable changes to **sheathe** are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/); this project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed
- Updated **`aes` to 0.9** (RustCrypto `cipher` 0.5; `generic-array` →
  `hybrid-array`). No behavioural change — AES-CTR/CBC/ECB outputs verified
  identical, MSRV unchanged (1.85).

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
- Versioning conformed to the template: explicit per-crate `version` (no shared
  `workspace.package.version`) and path-first internal dependency pins, so
  `scripts/release.sh` / `just release <version>` bumps every crate + pin in one
  pass. No published-crate changes.

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

[0.1.4]: https://github.com/vbasky/sheathe/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/vbasky/sheathe/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/vbasky/sheathe/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vbasky/sheathe/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vbasky/sheathe/releases/tag/v0.1.0
