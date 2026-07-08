<p align="center">
  <img src="https://raw.githubusercontent.com/vbasky/sheathe/main/docs/banner.png" alt="sheathe — pure-Rust HLS / DASH / CMAF packager" width="100%">
</p>

# sheathe

[![CI](https://github.com/vbasky/sheathe/actions/workflows/ci.yml/badge.svg)](https://github.com/vbasky/sheathe/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/sheathe.svg?logo=rust&label=crates.io)](https://crates.io/crates/sheathe)
[![docs.rs](https://img.shields.io/docsrs/sheathe?logo=docs.rs&label=docs.rs)](https://docs.rs/sheathe)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Medium](https://img.shields.io/badge/Medium-read%20the%20story-black?logo=medium)](https://medium.com/@vbasky/packaging-the-worlds-video-in-pure-rust-ff1f6b884fec)

**Pure-Rust HLS / DASH / CMAF media packager.** A memory-safe, dependency-light
alternative to [Shaka Packager](https://github.com/shaka-project/shaka-packager),
built and validated against it as the reference oracle.

📖 **Read the story:** [Packaging the World's Video in Pure Rust](https://medium.com/@vbasky/packaging-the-worlds-video-in-pure-rust-ff1f6b884fec)

> Status: **working VOD pipeline + encryption + a broad input matrix.** `probe`
> and `package` demux MP4, MPEG-TS, raw elementary streams, and WebM/Matroska,
> then write playable CMAF segments + DASH/HLS manifests with correct codec
> strings. Video: H.264, H.265, AV1, VP8/VP9. Audio: AAC, AC-3, E-AC-3, MP3,
> FLAC, Opus. Text: WebVTT, plus CEA-608 caption extraction from H.264/H.265 SEI.
> Encryption covers all four CENC schemes (`cenc`/`cens`/`cbc1`/`cbcs`) with
> multi-DRM `pssh` and key rotation. The path to full Shaka Packager parity (the
> last codec/format gaps, real-corpus oracle diffs, live) is tracked in
> [`ROADMAP.md`](./ROADMAP.md).

## Why

Mature DASH/HLS manifest *parsers* exist in Rust, but a mature *packager /
origin* does not. `sheathe` fills the Delivery lane: probe → ladder → CMAF
segment → DASH/HLS manifests, with no C/C++ dependencies.

## Workspace layout

| Crate | Role | Shaka Packager analogue |
| ------- | ------ | ------------------------- |
| [`sheathe-core`](crates/sheathe-core)     | Media model: streams, samples, timing, errors | `media/base` |
| [`sheathe-mp4`](crates/sheathe-mp4)       | ISO-BMFF / fMP4 / CMAF box writing + fragmentation | `media/formats/mp4` + chunking |
| [`sheathe-ts`](crates/sheathe-ts)         | MPEG-2 TS demux (PAT/PMT/PES) + audio codec parsers (AAC/AC-3/E-AC-3/MP3/FLAC) | `media/formats/mpeg` |
| [`sheathe-es`](crates/sheathe-es)         | Raw elementary stream demux (Annex B, ADTS, AC-3/E-AC-3, MP3, FLAC) | `media/formats` |
| [`sheathe-mkv`](crates/sheathe-mkv)       | WebM/Matroska (EBML) demux — VP8/VP9/AV1 + Opus | `media/formats/webm` |
| [`sheathe-text`](crates/sheathe-text)     | Timed text: WebVTT input + CEA-608 caption extraction → `wvtt` | `media/formats/webvtt` |
| [`sheathe-dash`](crates/sheathe-dash)     | MPEG-DASH `.mpd` generation | `mpd` |
| [`sheathe-hls`](crates/sheathe-hls)       | HLS master + media playlist generation | `hls` |
| [`sheathe-crypto`](crates/sheathe-crypto) | Common Encryption (cenc / cbcs) | `media/crypto` |
| [`sheathe`](crates/sheathe) / [`sheathe-cli`](crates/sheathe-cli) | The `sheathe` binary / its CLI library | `app` (`packager`) |

## Install / build

```sh
cargo install sheathe        # installs the `sheathe` binary
# or, from a checkout:
cargo run -p sheathe -- --help
```

## Usage (target CLI)

```sh
# Package an MP4 into 6s CMAF segments with both DASH and HLS manifests.
sheathe package input.mp4 --out site/ --segment-duration 6 --dash --hls

# Inspect what sheathe detects (MP4, MPEG-TS, WebM, raw ES, WebVTT).
sheathe probe input.mp4
sheathe probe input.ts
sheathe probe input.webm
sheathe package input.h264 input.ac3 --out site/ --dash --hls   # CEA-608 auto-extracted
sheathe package input.webm subtitles.vtt --out site/ --dash

# Differential-test against Shaka Packager (when `packager` is on PATH).
just oracle input.mp4
```

## Method

Per the workspace's `revelo method`: implement in pure Rust, then
differential-test output (segments, MPD, playlists) against Shaka Packager on a
sample corpus. Numbers and bitstreams that can't be validated against the oracle
don't ship.

## MSRV

Rust **1.85** (declared in `Cargo.toml`'s `workspace.package.rust-version`). CI
reads that exact value and builds against it, so the MSRV can't drift.

## License

`MIT OR Apache-2.0`.
