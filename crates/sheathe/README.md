# sheathe

**Pure-Rust HLS / DASH / CMAF media packager** — a memory-safe, dependency-light
alternative to [Shaka Packager](https://github.com/shaka-project/shaka-packager).

```sh
cargo install sheathe
sheathe package input.mp4 --out site/ --dash --hls
```

Demuxes MP4, writes CMAF init + media segments, and emits DASH
(`SegmentTimeline`) and HLS (master + media, audio rendition groups) with correct
RFC 6381 codec strings. CENC `cenc` / `cbcs` encryption via `--enc-key KID:KEY`.

See the [project repository](https://github.com/vbasky/sheathe) for the full
README, ROADMAP, and crate breakdown. Licensed under MIT OR Apache-2.0.
