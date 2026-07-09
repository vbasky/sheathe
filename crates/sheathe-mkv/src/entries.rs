//! Sample-entry synthesis for WebM/Matroska codecs.

/// Build a `VisualSampleEntry` box (`fourcc`) with a trailing config box.
fn visual_entry(fourcc: &[u8; 4], width: u16, height: u16, config: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let body_len = 78 + config.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&[0; 6]); // reserved
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 16]); // pre_defined + reserved + pre_defined[3]
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // horizresolution 72dpi
    out.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // vertresolution 72dpi
    out.extend_from_slice(&0u32.to_be_bytes()); // reserved
    out.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    out.extend_from_slice(&[0; 32]); // compressorname
    out.extend_from_slice(&0x0018u16.to_be_bytes()); // depth = 24
    out.extend_from_slice(&0xffffu16.to_be_bytes()); // pre_defined = -1
    out.extend_from_slice(config);
    out
}

/// Build an `AudioSampleEntry` box (`fourcc`) with a trailing config box.
fn audio_entry(fourcc: &[u8; 4], channels: u16, sample_rate: u32, config: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let body_len = 28 + config.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&[0; 6]); // reserved
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // reserved
    out.extend_from_slice(&channels.to_be_bytes());
    out.extend_from_slice(&16u16.to_be_bytes()); // samplesize
    out.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(sample_rate << 16).to_be_bytes());
    out.extend_from_slice(config);
    out
}

fn full_box(kind: &[u8; 4], version_flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + payload.len());
    out.extend_from_slice(&((12 + payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(&version_flags.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn plain_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// VP9 level index (Annex A) from the luma picture size. The full level also
/// depends on the luma sample *rate* (needs frame rate), but the picture-size
/// bound is what common encoders — and Shaka Packager — report here.
fn vp9_level(width: u16, height: u16) -> u8 {
    let pixels = u32::from(width) * u32::from(height);
    // (MaxLumaPicSize, level_idx) in ascending order.
    const TABLE: &[(u32, u8)] = &[
        (36_864, 10),
        (73_728, 11),
        (122_880, 20),
        (245_760, 21),
        (552_960, 30),
        (983_040, 31),
        (2_228_224, 40),
        (8_912_896, 50),
        (35_651_584, 60),
    ];
    TABLE.iter().find(|&&(max, _)| pixels <= max).map(|&(_, l)| l).unwrap_or(62)
}

/// `vp09` VisualSampleEntry + `vpcC`. Profile 0 / 8-bit / 4:2:0; the level is
/// derived from the picture size and the RFC 6381 codec string is emitted in
/// full (`vp09.PP.LL.DD.CC.cp.tc.mc.FR`) to match Shaka Packager.
pub(crate) fn vp9_entry(width: u16, height: u16) -> (Vec<u8>, String) {
    let level = vp9_level(width, height);
    let vpcc_payload = [
        0x00,                // profile 0
        level,               // level (from luma picture size)
        (8 << 4) | (1 << 1), // bitDepth=8, chromaSubsampling=1 (4:2:0), fullRange=0
        0x02,                // colourPrimaries = unspecified
        0x02,                // transferCharacteristics = unspecified
        0x02,                // matrixCoefficients = unspecified
        0x00,
        0x00, // codecInitializationDataSize
    ];
    let vpcc = full_box(b"vpcC", 1 << 24, &vpcc_payload); // version 1
    let codec = format!("vp09.00.{level:02}.08.01.02.02.02.00");
    (visual_entry(b"vp09", width, height, &vpcc), codec)
}

/// `vp08` VisualSampleEntry (VP8 carries no standard MP4 config box).
pub(crate) fn vp8_entry(width: u16, height: u16) -> (Vec<u8>, String) {
    (visual_entry(b"vp08", width, height, &[]), "vp08".to_string())
}

/// `av01` VisualSampleEntry + `av1C`. The Matroska `CodecPrivate` *is* the
/// AV1CodecConfigurationRecord, so it becomes the `av1C` payload verbatim.
pub(crate) fn av1_entry(codec_private: &[u8], width: u16, height: u16) -> (Vec<u8>, String) {
    let av1c = plain_box(b"av1C", codec_private);
    (visual_entry(b"av01", width, height, &av1c), "av01".to_string())
}

/// `Opus` AudioSampleEntry + `dOps`, built from the Matroska `CodecPrivate`
/// (an `OpusHead` identification header).
pub(crate) fn opus_entry(codec_private: &[u8], channels: u16) -> Option<(Vec<u8>, String)> {
    let dops = dops_from_opus_head(codec_private)?;
    // Opus always decodes at 48 kHz in ISO-BMFF.
    // Box type is `Opus`; the RFC 6381 codecs value is lowercase `opus`.
    Some((audio_entry(b"Opus", channels, 48_000, &dops), "opus".to_string()))
}

/// Translate an `OpusHead` (little-endian) into a `dOps` box (big-endian).
fn dops_from_opus_head(head: &[u8]) -> Option<Vec<u8>> {
    if head.len() < 19 || &head[0..8] != b"OpusHead" {
        return None;
    }
    let channel_count = head[9];
    let pre_skip = u16::from_le_bytes([head[10], head[11]]);
    let input_sample_rate = u32::from_le_bytes([head[12], head[13], head[14], head[15]]);
    let output_gain = i16::from_le_bytes([head[16], head[17]]);
    let mapping_family = head[18];

    let mut payload = Vec::new();
    payload.push(0); // dOps version
    payload.push(channel_count);
    payload.extend_from_slice(&pre_skip.to_be_bytes());
    payload.extend_from_slice(&input_sample_rate.to_be_bytes());
    payload.extend_from_slice(&output_gain.to_be_bytes());
    payload.push(mapping_family);
    if mapping_family != 0 {
        // StreamCount, CoupledCount, ChannelMapping[channel_count].
        payload.extend_from_slice(head.get(19..19 + 2 + usize::from(channel_count))?);
    }
    Some(plain_box(b"dOps", &payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vp9_entry_has_vpcc() {
        let (entry, codec) = vp9_entry(1280, 720);
        assert_eq!(&entry[4..8], b"vp09");
        assert!(entry.windows(4).any(|w| w == b"vpcC"));
        // 1280×720 = 921600 luma samples → level 3.1 (31).
        assert_eq!(codec, "vp09.00.31.08.01.02.02.02.00");
    }

    #[test]
    fn av1_entry_wraps_codec_private() {
        let cp = [0x81, 0x00, 0x0c, 0x00]; // dummy AV1 config record
        let (entry, _) = av1_entry(&cp, 640, 360);
        assert_eq!(&entry[4..8], b"av01");
        let i = entry.windows(4).position(|w| w == b"av1C").unwrap();
        assert_eq!(&entry[i + 4..i + 8], &cp);
    }

    #[test]
    fn opus_entry_builds_dops() {
        // OpusHead: magic, ver1, 2ch, preskip 312, rate 48000, gain 0, family 0.
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(2);
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        let (entry, codec) = opus_entry(&head, 2).expect("opus");
        assert_eq!(&entry[4..8], b"Opus");
        let i = entry.windows(4).position(|w| w == b"dOps").unwrap();
        // dOps payload starts right after the 4-byte fourcc.
        assert_eq!(entry[i + 4], 0); // version
        assert_eq!(entry[i + 5], 2); // channels
        // pre_skip 312 big-endian.
        assert_eq!(u16::from_be_bytes([entry[i + 6], entry[i + 7]]), 312);
        assert_eq!(codec, "opus");
    }
}
