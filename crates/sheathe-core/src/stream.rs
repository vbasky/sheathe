//! Elementary-stream description: what a track *is*, independent of container.

use crate::time::Timescale;

/// The broad category of an elementary stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Video / image sequence.
    Video,
    /// Audio.
    Audio,
    /// Timed text / subtitles / captions.
    Text,
}

/// The codec carried by a stream. The string in [`Codec::Other`] is a
/// best-effort fourcc/codec id for formats not yet first-classed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
    Av1,
    Vp8,
    Vp9,
    Aac,
    Ac3,
    Eac3,
    Mp3,
    Flac,
    Opus,
    WebVtt,
    /// TTML / IMSC 1.0/1.1 timed text (`stpp`).
    Stpp,
    /// Anything else, keyed by its fourcc or registration string.
    Other(String),
}

impl Codec {
    /// The RFC 6381 `codecs=` prefix used in DASH/HLS manifests.
    pub fn rfc6381_family(&self) -> &str {
        match self {
            Codec::H264 => "avc1",
            Codec::H265 => "hvc1",
            Codec::Av1 => "av01",
            Codec::Vp8 => "vp08",
            Codec::Vp9 => "vp09",
            Codec::Aac => "mp4a",
            Codec::Ac3 => "ac-3",
            Codec::Eac3 => "ec-3",
            Codec::Mp3 => "mp4a",
            Codec::Flac => "fLaC",
            Codec::Opus => "Opus",
            Codec::WebVtt => "wvtt",
            Codec::Stpp => "stpp",
            Codec::Other(s) => s.as_str(),
        }
    }
}

/// Everything the packager needs to know about one elementary stream.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Video / audio / text.
    pub kind: MediaKind,
    /// The codec carried.
    pub codec: Codec,
    /// The stream's media timescale.
    pub timescale: Timescale,
    /// Width/height in pixels for video; `None` otherwise.
    pub resolution: Option<(u32, u32)>,
    /// Sample rate in Hz for audio; `None` otherwise.
    pub sample_rate: Option<u32>,
    /// Average bitrate in bits/sec, if known.
    pub bitrate: Option<u32>,
    /// Full RFC 6381 `codecs=` string (e.g. `avc1.640028`, `mp4a.40.2`), if the
    /// codec configuration was parsed. Falls back to the codec family otherwise.
    pub codec_string: Option<String>,
}

impl StreamInfo {
    /// The RFC 6381 `codecs=` value for this stream: the parsed
    /// [`StreamInfo::codec_string`] if present, else the bare codec family.
    pub fn rfc6381(&self) -> String {
        self.codec_string.clone().unwrap_or_else(|| self.codec.rfc6381_family().to_string())
    }
}
