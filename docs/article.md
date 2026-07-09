# Packaging the World's Video in Pure Rust

## Building sheathe, an HLS/DASH/CMAF media packager, validated byte-for-byte against Shaka Packager

Every video you stream on YouTube, Netflix, or an OTT sports app arrives in pieces. A two-hour movie is not one file served in order — it is a manifest listing hundreds of small segments, each containing a few seconds of video and audio, joined by a description that tells the player how to stitch them back together. The tool that makes those pieces is called a *packager*, and production streaming has relied on exactly one open-source implementation for a decade: Google's Shaka Packager.

This is the story of building a second one — in Rust, with no C or C++ anywhere in the dependency tree, validated against the original byte-for-byte.

---

## What a Packager Actually Does

A packager takes a finished media file — say, a 1080p MP4 with H.264 video and AAC audio — and produces what the web needs to stream it:

- **CMAF segments.** Common Media Application Format fragments, the ISO-BMFF-based chunks that every modern player understands. Each segment is a self-contained fMP4 file with an initialization header plus a sequence of `moof`+`mdat` fragments covering ~6 seconds.
- **A DASH manifest.** An MPD (Media Presentation Description) XML document that lists every segment URL, its duration, its resolution, and its codec. A player reads the MPD, figures out which rendition matches the current network, and fetches segments.
- **An HLS playlist.** A master `.m3u8` listing available renditions plus per-track media playlists, with the same segment references. Older players and Apple devices use this path.

If the content is encrypted — and in production it always is — the packager also applies Common Encryption (CENC), embeds the DRM signalling, and writes the per-sample encryption metadata that a player's Content Decryption Module needs to decrypt each frame.

That is the job. The reference implementation — Shaka Packager — does it in C++ with ~250,000 lines of code, covering a dozen input formats, four encryption schemes, live streaming, trick-play, and HLS packed-audio output. The goal of sheathe is functional parity on the subset that production VOD (Video on Demand) actually uses, done in pure Rust and differential-tested against Shaka at every step.

---

## Why Build Another Packager?

There are excellent Rust projects that parse DASH manifests and HLS playlists. There was no mature Rust project that *produces* segments and manifests from video files — the write side is the hard side, and the ecosystem had a gap in the Delivery lane: probe the source, build the ladder, segment into CMAF, write the manifests.

sheathe fills that gap. It also fills a subtler one: the standard packager is a native binary written in C++, carrying all the memory-safety risk of parsing untrusted media containers at scale. A packager that does the same work in `unsafe`-free Rust turns a class of buffer-overflow CVEs into compile-time impossibilities. That matters the moment you point it at a file you didn't create.

---

## Architecture: Nine Crates, Four Layers

sheathe is nine crates arranged in four layers, each crate with one responsibility:

```
Foundation   sheathe-core      Media model: streams, samples, timing, errors
             sheathe-mp4        ISO-BMFF box reader/writer, CMAF fragments
             sheathe-crypto     CENC encryption (all four schemes + DRM pssh)
             sheathe-ts         MPEG-2 transport stream demux (PAT/PMT/PES)
             sheathe-es         Raw elementary stream demux (Annex B, ADTS)

Pipeline     sheathe-dash       DASH MPD generation
             sheathe-hls        HLS master + media playlist generation

Application  sheathe-cli        The `sheathe` binary
             sheathe            CLI crate (thin wrapper over sheathe-cli)
```

The dependency graph points strictly down. A manifest crate composes foundation crates but never reaches sideways into another manifest crate. This is where Rust's crate boundary earns its keep: the MP4 writer cannot accidentally call into the DASH MPD generator, because the compiler won't let it. Nine shallow crates, each small enough to hold in your head and test in isolation.

The whole workspace is ~6,000 lines of Rust across 41 source files, with 34 tests that verify everything from AES-128-CTR NIST vectors to AVC codec-string derivation — and an integration harness that diffs output against Shaka Packager itself.

---

## The Read Side: Demuxing MP4, MPEG-TS, and Elementary Streams

Before sheathe can write CMAF segments, it has to read the source. Three input paths are wired:

**MP4 demux (`sheathe-mp4`).** A dependency-free, cursor-based box reader traverses the ISO-BMFF hierarchy: `moov` → `trak` → `mdia` → `minf` → `stbl`. From the sample table (`stts`/`ctts`/`stsc`/`stsz`/`stco`/`stss`) it reconstructs every sample's decode time, composition offset, size, offset, and keyframe flag — the exact information the fragmenter needs to slice the stream into segments. The box reader is a simple state machine over a `&[u8]` reference; there is no allocation beyond the `BoxIter` stack, and no `unsafe`.

```rust
// Top-level box traversal: find every box at the current nesting level,
// yielding (type, body, next_offset) for each.
pub fn top_level(data: &[u8]) -> impl Iterator<Item = std::io::Result<Mp4Box<'_>>> {
    // ... purely a Cursor over the byte slice
}
```

**MPEG-TS demux (`sheathe-ts`).** Transport streams are a different beast entirely — 188-byte packets carrying program-specific information (PAT/PMT) and packetized elementary stream (PES) payloads. `sheathe-ts` parses PAT to find the program, PMT to find the streams, PES headers to reassemble access units, and Annex B / ADTS parsers to extract H.264, HEVC, and AAC samples. It synthesizes `avcC`/`hvcC`/`mp4a` sample entries from the elementary stream headers, so the output segment's `moov` carries the decoder configuration the player needs — all without an external probe tool.

**Elementary stream demux (`sheathe-es`).** For raw `.h264`, `.hevc`, and `.aac` files, sheathe detects the format from the extension and content signature, splits the stream into access units (via Annex B start-code scanning for video, ADTS frame sync for audio), and feeds them to the same fragmenter. No container, no problem.

---

## The Write Side: CMAF Segments from a Stream of Samples

Once the demuxer has produced a stream of `Sample` structs (data, decode timestamp, composition offset, duration, keyframe flag), the fragmenter decides where to cut. The policy is the same one Shaka uses: wait until the accumulated samples exceed the target segment duration (default 6 seconds), then cut at the next keyframe — because a segment that starts on a non-keyframe cannot be decoded independently, and DASH/HLS switching depends on clean boundaries.

```rust
pub fn push(&mut self, sample: Sample) -> Result<()> {
    let elapsed = sample.dts.saturating_sub(self.current_start);
    let may_cut = !self.policy.keyframes_only || sample.is_segment_boundary();
    if !self.current.is_empty() && may_cut && elapsed >= self.target_ticks {
        self.cut(sample.dts);
    }
    if self.current.is_empty() {
        self.current_start = sample.dts;
    }
    self.current.push(sample);
    Ok(())
}
```

Each segment is written as an fMP4 file: a `styp` box (segment type), a `sidx` box (segment index for seeking), a `moof` box (movie fragment — the sample table for this segment), and an `mdat` box (the actual sample data). The init segment carries `ftyp` + `moov` + `mvex`, describing the codec configuration once, at the start.

The box writer is a 75-line nestable builder with automatic size backpatching — no pre-computed table of contents. `begin("moof")`, write child boxes, `end()` — and the 32-bit size in the header is patched to the correct value. This is the same pattern Shaka uses; sheathe just does it in 1/10th the code.

---

## The Hard Part: Common Encryption Done Correctly

Encryption is where most packagers get complex. CENC (ISO/IEC 23001-7) defines four protection schemes, and sheathe implements all of them:

| Scheme | Cipher | Mode | Video Pattern | Audio |
|--------|--------|------|---------------|-------|
| `cenc` | AES-128-CTR | Full-region | NAL-aware subsamples | Whole-sample |
| `cens` | AES-128-CTR | Pattern (1:9) | Crypt 1 block, skip 9 | Whole-sample |
| `cbc1` | AES-128-CBC | Full-region | Per-sample IV, block-aligned | Whole-sample |
| `cbcs` | AES-128-CBC | Pattern (1:9) | Crypt 1, skip 9, constant IV | Whole-sample |

The `sheathe-crypto` crate is a 457-line module built on the RustCrypto `aes` crate. The AES-128-CTR path runs a continuous keystream across protected byte ranges, skipping clear bytes and pattern-skipped blocks without advancing the counter. The AES-128-CBC path chains ciphertext blocks, resetting to the constant IV at each subsample boundary under pattern encryption. Both are validated against NIST SP 800-38A test vectors, and against Shaka Packager's output on real media.

Every scheme is ffmpeg decrypt+decode verified. That is: sheathe encrypts a sample, produces an fMP4 segment, and then `ffmpeg -decryption_key ... -i segment.mp4 -f framemd5` confirms every video frame and every audio frame decodes to the same MD5 as the clear original. No "probably encrypted" — provably correct.

### Multi-DRM pssh

Encrypting the bytes is half the job; the other half is telling the player's DRM module *how* to decrypt them. Each DRM system gets a `pssh` box in the init segment, generated directly from the raw key (no key server required):

- **Common** — a version-1 box listing the KID, used by W3C clear-key and `urn:mpeg:dash:mp4protection`.
- **Widevine** — a version-0 box carrying a `WidevinePsshData` protobuf with the KID and protection-scheme fourcc.
- **PlayReady** — a version-0 box wrapping a PlayReady Object with a `WRMHEADER` 4.0.0.0 (KID, key length, ALGID, and an AES-ECB checksum proving key possession).

Each box byte-matches Shaka Packager's `--protection_systems` output. Not approximately — identically.

### Key Rotation

For long content, rotating keys reduces the exposure window of any single key. sheathe supports `--crypto-period-duration <seconds>`: each period derives a new content key by left-rotating the base key (Shaka's naive raw-key scheme), signalled per segment via `seig` sample groups (`sbgp`/`sgpd`), a zero-KID init `tenc`, and a per-period `pssh` in each `moof`. Every segment decrypts to the clear baseline under its derived key.

---

## Manifest Generation: DASH and HLS

With segments written, the packager generates the manifests that players consume.

**DASH.** The MPD is an XML document in the static (`type="static"`), on-demand profile, using `SegmentTemplate` + `SegmentTimeline` — the same profile Shaka produces for VOD. Each representation gets a `<Representation>` element with its codec string (RFC 6381), resolution, bandwidth, and a `<SegmentTimeline>` that lists each segment's exact duration. Equal-duration runs are collapsed with `r=` (repeat count), so a 2-hour movie doesn't produce 1,200 `<S>` elements.

The codec strings themselves are derived from raw box data — `avcC` → `avc1.640028`, `hvcC` → `hvc1.1.6.L93.90`, `av1C` → `av01.0.04M.08`, `esds` → `mp4a.40.2` — in a pure-Rust, zero-dependency codec-string module with tests pinned against known outputs.

```rust
// avc1.PPCCLL from avcC
fn avc_string(fourcc: &[u8; 4], avcc: &[u8]) -> Option<String> {
    let (profile, compat, level) = (*avcc.get(1)?, *avcc.get(2)?, *avcc.get(3)?);
    let prefix = std::str::from_utf8(fourcc).ok()?;
    Some(format!("{prefix}.{profile:02x}{compat:02x}{level:02x}"))
}
```

**HLS.** The master playlist lists video variants as `#EXT-X-STREAM-INF` entries with combined bandwidth and codecs (video + audio folded together, per the spec), bound to an `#EXT-X-MEDIA` audio rendition group. Media playlists carry `#EXTINF` per-segment durations, `#EXT-X-MAP` pointing to the init segment, and `#EXT-X-KEY` encryption signalling (`SAMPLE-AES` for cbcs, `SAMPLE-AES-CTR` for cenc) — the full HLS encrypted-fMP4 contract.

---

## The Oracle: Differential Testing Against Shaka Packager

sheathe adopts the "revelo method": implement in pure Rust, then differential-test against a reference oracle. For a packager, the oracle is Shaka Packager running the same input file through the same output configuration.

The oracle harness (`just oracle input.mp4`) runs both packagers on the same source, then diffs:

1. **Segment counts.** Do sheathe and Shaka produce the same number of media segments?
2. **Canonical MPD.** XML canonicalization (`xmllint --c14n`) strips formatting to compare structure — same representations, same timelines, same codec strings.
3. **ffmpeg decode.** Both outputs are fed through `ffmpeg -f framemd5` to confirm the video renders identically.

This is the same methodology that keeps revelo's 180+ parsers byte-identical to MediaInfoLib and viser's convex hulls correct to the numerical method. Nothing ships until its bytes are verified against the oracle. If Shaka and sheathe disagree, the bug is in sheathe until proven otherwise.

---

## What Makes This Genuinely Hard

Rust makes it safe; the domain makes it hard. The genuinely difficult parts of building a packager are worth naming:

**NAL-aware subsample encryption is format-specific.** AVC and HEVC samples cannot be encrypted blindly — the NAL unit headers and slice headers must stay clear for a player to parse the stream. The `senc` box carries a table of (clear_bytes, encrypted_bytes) per sample, and getting the boundary exactly right requires knowing the NAL unit length size from the `avcC`/`hvcC` configuration. Get it wrong, and the player produces a green screen or a crash.

**The full CENC scheme matrix has subtle interactions.** `cbcs` uses a constant IV across all samples; the other three derive per-sample IVs. `cbc1` chains CBC across subsamples; `cbcs` resets to the constant IV at each subsample. Pattern encryption applies to video only — audio under `cens`/`cbcs` is whole-sample encrypted with `Pattern::NONE`. DASH-IF conformance requires `saiz`/`saio` auxiliary-information boxes with the `saio` offset backpatched to point at the `senc` data. Every one of these interactions earned its own differential test.

**PlayReady pssh is deeply non-obvious.** The PlayReady Object wraps a `WRMHEADER` with a KID that uses swapped GUID byte order (the first three fields of the UUID are written little-endian, not network order), and an AES-ECB checksum of the object. Building this from the raw key requires implementing a subset of the PlayReady binary format that no RFC describes — the oracle is the only spec.

**The box model rewards getting it right the first time.** ISO-BMFF boxes nest arbitrarily: `moov` contains `trak`, which contains `mdia`, which contains `minf`, which contains `stbl`, which contains `stsd`, which contains the actual sample entry with its decoder configuration. A missing box, a wrong offset, or an incorrect size computation produces a file that looks valid to a file browser and silently fails to play. The reader and writer together form a round-trip that is verified on every test asset.

---

## The Result

sheathe is a pure-Rust HLS/DASH/CMAF media packager at ~6,000 lines, validated against Shaka Packager as the reference oracle. It demuxes MP4, MPEG-TS, and raw elementary streams; writes CMAF init and media segments with correct box structure and codec strings; applies all four CENC encryption schemes with multi-DRM pssh (Widevine, PlayReady, Common) and key rotation; and emits DASH MPDs and HLS playlists that match Shaka's output.

```sh
# Package an MP4 into 6s CMAF segments with both DASH and HLS manifests.
sheathe package input.mp4 --out site/ --segment-duration 6 --dash --hls

# Encrypt with CENC cbcs (AES-CBC pattern) and Widevine+PlayReady DRM.
sheathe package input.mp4 --out site/ --dash --hls \
  --enc-scheme cbcs \
  --enc-key abcd1234...:deadbeef... \
  --protection-systems common,widevine,playready

# Probe an MPEG-TS file — no ffprobe required.
sheathe probe input.ts

# Install the binary from crates.io.
cargo install sheathe
```

sheathe doesn't yet do live streaming, trick-play, or WebM input. Those are tracked on the roadmap, each gated behind an oracle validation checkpoint. What ships today is the complete VOD pipeline — the subset that covers ~90% of production packaging work — built in unsafe-free Rust, with no C dependencies, and every output differential-tested against the best open-source packager available.

---

## One More Thing: The Pure-Rust Probe

A packager needs to know what it's packaging. Resolution, frame rate, codec, bitrate — the numbers that determine how segments get sized and manifests get filled. The default way to get those numbers is to shell out to ffprobe.

sheathe doesn't need to. Its MP4 demuxer reads sample-table metadata directly from the box hierarchy; its MPEG-TS demuxer reconstructs stream descriptions from PAT/PMT/PES headers; its elementary-stream path synthesizes sample entries from the Annex B/ADTS byte stream itself. The `sheathe probe` command returns the same information ffprobe does, with no external binary on the path.

For the full metadata picture — the kind that covers 177+ media formats with byte-identical output to MediaInfoLib — there's [revelo](https://github.com/vbasky/revelo), another pure-Rust project that integrates as a drop-in probe engine. But for the subset a packager actually needs, sheathe is self-contained: the tool that segments video is also the tool that reads it. One binary, one dependency chain, one `cargo install`.

---

## What I'd Want You to Take From This

If there's a lesson worth carrying out of this project, it's simpler than the topic implies:

1. **Choose an oracle you can diff against.** Shaka Packager is open source, battle-tested on billions of streams, and emits deterministic output. Every feature sheathe ships was validated by running both tools on the same input and comparing the result. Byte-identical output is the only claim worth making.

2. **The crate boundary is a design tool.** Nine crates, each with exactly one concern, separated by the compiler. The MP4 writer cannot reach into the DASH generator; the crypto crate knows nothing about the container format. Wide and flat, not deep and tangled.

3. **The hardest bugs are interactions, not algorithms.** AES-CTR is 40 lines. Getting the `saiz` offset to backpatch correctly after the `senc` is written — that's where the hours go. The differential harness catches these; no amount of code review does.

4. **Packaging is infrastructure, not magic.** A packager is a program that reads video, cuts it into pieces, and writes a description of the pieces. Doing it correctly is a lot of work; doing it in pure Rust is less work than you'd think, and the result is safer, simpler, and faster to install than the C++ alternative.

sheathe is open source at [github.com/vbasky/sheathe](https://github.com/vbasky/sheathe), under MIT OR Apache-2.0.

*The author is also building [revelo](https://github.com/vbasky/revelo) (a pure-Rust MediaInfoLib port) and [viser](https://github.com/vbasky/viser) (a content-adaptive video encoding optimizer). The three projects form a suite: probe → ladder → package — the Delivery lane of a pure-Rust media ecosystem, end to end.*
