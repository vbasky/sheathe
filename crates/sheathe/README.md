<p align="center">
  <img src="https://raw.githubusercontent.com/vbasky/sheathe/main/docs/banner.png" alt="sheathe — pure-Rust HLS / DASH / CMAF packager" width="100%">
</p>

# sheathe

**Facade crate** for the [sheathe](https://github.com/vbasky/sheathe) workspace.
It re-exports the packaging pipeline and each format crate so you can depend on
a single `sheathe` crate instead of a dozen `sheathe-*` crates.

The command-line tool lives in the separate [`sheathe-cli`](https://crates.io/crates/sheathe-cli)
crate, which installs a `sheathe` binary:

```sh
cargo install sheathe-cli
```

See the [project README](https://github.com/vbasky/sheathe#readme) for commands,
recipes, and oracle status.

## Install

```toml
[dependencies]
sheathe = "0.6"
```

Or via Cargo:

```sh
cargo add sheathe
```

## Quick start

```rust,no_run
use sheathe::{package, PackageOptions};
use std::path::PathBuf;

let opts = PackageOptions {
    out_dir: PathBuf::from("out"),
    dash: true,
    hls: true,
    segment_duration: 6.0,
    ..Default::default()
};
let output = package(&[PathBuf::from("movie.mp4")], &opts).unwrap();
```

Lower-level crates are re-exported as modules (`sheathe::mp4`, `sheathe::dash`,
`sheathe::hls`, …) — see the [crate docs](https://docs.rs/sheathe).

## License

`MIT OR Apache-2.0`.
