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

> Status: **Phases 0–5 complete** — VOD + live/advanced manifests + on-demand
> single-file DASH, CMAF/TS/packed-audio segments, encryption, broad inputs, HTTP
> push, UDP ingest, and a JIT origin. `probe` / `package` / `origin` demux MP4,
> MPEG-TS, WebM/Matroska, and elementary streams; write DASH/HLS with correct
> codec strings. Video: H.264, H.265, AV1, VP8/VP9. Audio: AAC, AC-3, E-AC-3,
> MP3, FLAC, Opus. Text: WebVTT + CEA-608/708. CENC matrix + multi-DRM `pssh`.
> See [`ROADMAP.md`](./ROADMAP.md) and [`docs/CONFORMANCE.md`](./docs/CONFORMANCE.md).

## Why

Mature DASH/HLS manifest *parsers* exist in Rust, but a mature *packager /
origin* does not. `sheathe` fills the Delivery lane: probe → ladder → CMAF
segment → DASH/HLS manifests, with no C/C++ dependencies.

## Workspace layout

| Crate | Role | Shaka Packager analogue |
| ------- | ------ | ------------------------- |
| [`sheathe-core`](crates/sheathe-core)     | Media model: streams, samples, timing, errors | `media/base` |
| [`sheathe-mp4`](crates/sheathe-mp4)       | ISO-BMFF / fMP4 / CMAF box writing + fragmentation | `media/formats/mp4` + chunking |
| [`sheathe-ts`](crates/sheathe-ts)         | MPEG-2 TS demux + mux (PAT/PMT/PES) + audio parsers (AAC/AC-3/E-AC-3/MP3/FLAC) | `media/formats/mpeg` |
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

## Commands

| Command | Description |
|---------|-------------|
| `sheathe package` | Demux → fragment → CMAF/TS/packed-audio segments + DASH/HLS |
| `sheathe probe` | List streams sheathe detects (no packaging) |
| `sheathe origin` | JIT HTTP origin — package on `GET /package?input=…` |

**Full flag reference, recipes, and output layouts:**
[**docs/CLI.md**](./docs/CLI.md)

### Quick start

```sh
# VOD: CMAF segments + DASH + HLS
sheathe package input.mp4 -o site/ --dash --hls --segment-duration 6

# Inspect streams
sheathe probe input.mp4
sheathe probe input.ts
sheathe probe input.webm

# ABR ladder (each file = one rendition)
sheathe package v360.mp4 v720.mp4 v1080.mp4 -o ladder/ --dash --hls

# Live-style window from a finished mezzanine
sheathe package mezz.mp4 -o live/ --dash --hls \
  --presentation live --live-window 3

# Encrypted (cenc) multi-DRM
sheathe package in.mp4 -o secure/ --dash --hls \
  --enc-key 00112233445566778899aabbccddeeff:000102030405060708090a0b0c0d0e0f \
  --protection-systems common,widevine,playready

# On-demand single-file DASH
sheathe package in.mp4 -o od/ --dash --on-demand

# MPEG-TS HLS
sheathe package in.mp4 -o ts/ --hls --format ts

# Trick-play + low-latency + SCTE-35 ad markers
sheathe package in.mp4 -o advanced/ --dash --hls \
  --trick-play --low-latency --part-duration 0.5 \
  --scte35 30:out:15 --scte35 45:in

# JIT origin (package on request)
sheathe origin --bind 127.0.0.1:8787 --media-root .
# curl 'http://127.0.0.1:8787/package?input=clip.mp4&format=hls'
```

### `package` flag groups (summary)

| Group | Flags |
|-------|--------|
| Core | `-o/--out`, `--segment-duration`, `--dash`, `--hls` |
| Format | `--format cmaf\|ts\|packed-audio`, `--on-demand`, `--parallel`, `--http-push` |
| Presentation | `--presentation vod\|event\|live`, `--live-window`, `--multi-period` |
| Advanced | `--trick-play`, `--low-latency`, `--part-duration`, `--scte35`, `--availability-start-time` |
| Encryption | `--enc-key`, `--enc-key-file`, `--enc-scheme`, `--enc-key-uri`, `--protection-systems`, `--crypto-period-duration` |

See [docs/CLI.md](./docs/CLI.md) for defaults, output directory layout, and
every recipe (ABR, multi-period, DRM, LL-HLS, origin, push, oracle).

### Developer tasks (`just`)

```sh
just check-all              # fmt + clippy + test + docs
just oracle input.mp4       # differential vs Shaka Packager
just oracle-corpus          # full real-media corpus gate
just bench                  # throughput vs Shaka (optional)
just corpus                 # fetch checksum-pinned test media
```

## Documentation map

| Doc | Contents |
|-----|----------|
| [docs/CLI.md](./docs/CLI.md) | **Command reference** — all subcommands, flags, recipes |
| [docs/CONFORMANCE.md](./docs/CONFORMANCE.md) | Oracle gates, DASH-IF / mediastreamvalidator, fuzz |
| [ROADMAP.md](./ROADMAP.md) | Phase status (0–5 complete) |
| [CHANGELOG.md](./CHANGELOG.md) | Release notes |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | Dev workflow, hooks, style |

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
