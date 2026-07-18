# Conformance & quality gates

sheathe follows the **revelo method**: features are not “done” until their
bytes are checked against an oracle (Shaka Packager) and, where useful,
independent tools.

## Built-in gates

| Gate | Command | What it checks |
|------|---------|----------------|
| Unit / integration | `just test` | Hermetic synthetic fixtures per crate |
| Clippy + fmt | `just check-all` | Style + correctness lints |
| Shaka differential | `just oracle <input>` | CMAF box structure, MPD, HLS vs Shaka |
| Real corpus | `just oracle-corpus` | Decode + Shaka + CEA-608 vs ccextractor |
| Throughput | `./scripts/bench_throughput.sh` | Wall-clock package vs Shaka |
| Fuzz (optional) | `cargo +nightly fuzz run mp4_box_reader` | Demuxer crash resistance |

## External validators (optional, not required in CI)

These tools are **not** vendored. Run them on packaged output when preparing a
release or validating a customer asset.

### DASH-IF Conformance

1. Package: `sheathe package in.mp4 -o out --dash --hls`
2. Upload `out/manifest.mpd` + segments to the [DASH-IF Conformance Tool](https://conformance.dashif.org/)
   or run a local validator if you maintain one.
3. On-demand profile: add `--on-demand` and validate `isoff-on-demand` profile.

### Apple `mediastreamvalidator`

```bash
sheathe package in.mp4 -o out --hls --dash
mediastreamvalidator out/master.m3u8
```

Requires Apple’s HLS tools (macOS). Useful for `#EXT-X-KEY`, I-frame playlists,
and LL-HLS tag shape.

### Widevine / PlayReady test vectors

Encryption is oracle-verified against Shaka Packager’s `pssh` / `tenc` / `senc`
bytes (`just oracle` with `--enc-key`). Full DRM license-server integration is
**out of scope** for hermetic CI (requires client certs + live endpoints).

## Fuzzing

```bash
# Requires nightly + cargo-fuzz
cargo install cargo-fuzz
cargo +nightly fuzz run mp4_box_reader -- -max_total_time=60
cargo +nightly fuzz run ts_packet -- -max_total_time=60
cargo +nightly fuzz run mkv_ebml -- -max_total_time=60
```

Targets live under `fuzz/fuzz_targets/`.
