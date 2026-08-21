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
| `sheathe probe` | Dump stream info (Shaka `--dump_stream_info`) without packaging |
| `sheathe origin` | JIT HTTP(S) origin — package on `GET /package?input=…` |

```sh
sheathe package input.mp4 -o site/ --dash --hls --segment-duration 6
sheathe package in=movie.mp4,stream=audio,language=eng in=movie.mp4,stream=video -o site/ --dash --hls
sheathe probe input.mp4
sheathe origin --bind 127.0.0.1:8787 --media-root .
sheathe origin --bind 127.0.0.1:8443 --tls-cert cert.pem --tls-key key.pem --auth user:pass
```

`--help` prints Shaka-style flag docs (HLS/DASH knobs, CPIX/Widevine, HTTPS
push, live rewrite). Full reference: [`docs/CLI.md`](https://github.com/vbasky/sheathe/blob/main/docs/CLI.md).

## Library

Application code should depend on the [`sheathe`](https://crates.io/crates/sheathe)
facade crate (`cargo add sheathe`), not this CLI crate.

## License

`MIT OR Apache-2.0`.
