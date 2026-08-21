<p align="center">
  <img src="https://raw.githubusercontent.com/vbasky/sheathe/main/docs/banner.png" alt="sheathe — pure-Rust HLS / DASH / CMAF packager" width="100%">
</p>

# sheathe-cli

CLI for [sheathe](https://github.com/vbasky/sheathe) — a pure-Rust HLS / DASH /
CMAF media packager, validated against
[Shaka Packager](https://github.com/shaka-project/shaka-packager) as the
reference oracle.

The binary is named `sheathe`.

See the [project README](https://github.com/vbasky/sheathe#readme) and
[`docs/CLI.md`](https://github.com/vbasky/sheathe/blob/main/docs/CLI.md) for
the full command reference.

## Install

```sh
cargo install sheathe-cli
```

## Commands

| Command | Description |
|---------|-------------|
| `sheathe package` | Demux → fragment → CMAF/TS/packed-audio segments + DASH/HLS |
| `sheathe probe` | List streams sheathe detects (no packaging) |
| `sheathe origin` | JIT HTTP origin — package on `GET /package?input=…` |

```sh
sheathe package input.mp4 -o site/ --dash --hls --segment-duration 6
sheathe probe input.mp4
sheathe origin --bind 127.0.0.1:8787 --media-root .
```

## Library

Application code should depend on the [`sheathe`](https://crates.io/crates/sheathe)
facade crate (`cargo add sheathe`), not this CLI crate.

## License

`MIT OR Apache-2.0`.
