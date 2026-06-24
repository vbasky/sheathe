//! HLS (RFC 8216) playlist generation for **sheathe**.
//!
//! Mirrors Shaka Packager's `hls` library: emits a master playlist plus one
//! media playlist per rendition, referencing the CMAF segments produced by
//! [`sheathe-mp4`]. Output is differential-tested against Shaka Packager.
//!
//! [`sheathe-mp4`]: https://crates.io/crates/sheathe-mp4

use sheathe_core::{MediaKind, StreamInfo};
use std::fmt::Write as _;

/// One variant stream listed in the master playlist.
#[derive(Debug, Clone)]
pub struct Variant {
    /// The stream this variant carries.
    pub stream: StreamInfo,
    /// Path to this variant's media playlist, relative to the master.
    pub playlist_uri: String,
}

/// A single media segment line in a media playlist.
#[derive(Debug, Clone)]
pub struct SegmentRef {
    /// Segment duration in seconds (`#EXTINF`).
    pub duration: f64,
    /// Segment URI.
    pub uri: String,
}

/// HLS audio rendition group id used to bind video variants to audio.
const AUDIO_GROUP: &str = "aud";

/// Build a master playlist over `variants`.
///
/// Audio variants are emitted as an `#EXT-X-MEDIA` rendition group; video
/// variants become `#EXT-X-STREAM-INF` entries that reference it (with the audio
/// codec folded into `CODECS` and bandwidth). With no video, audio variants fall
/// back to plain `#EXT-X-STREAM-INF` so the playlist is still usable.
pub fn master_playlist(variants: &[Variant]) -> String {
    let audio: Vec<&Variant> = variants
        .iter()
        .filter(|v| v.stream.kind == MediaKind::Audio)
        .collect();
    let video: Vec<&Variant> = variants
        .iter()
        .filter(|v| v.stream.kind == MediaKind::Video)
        .collect();

    let mut s = String::from("#EXTM3U\n#EXT-X-VERSION:7\n");

    for (i, a) in audio.iter().enumerate() {
        let default = if i == 0 { "YES" } else { "NO" };
        let _ = writeln!(
            s,
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"{AUDIO_GROUP}\",NAME=\"audio{i}\",\
             DEFAULT={default},AUTOSELECT=YES,URI=\"{}\"",
            a.playlist_uri
        );
    }

    let has_audio = !audio.is_empty();
    let audio_extra = audio.first();

    if video.is_empty() {
        // Audio-only: list audio renditions as plain variants too.
        for a in &audio {
            stream_inf(&mut s, a, None);
        }
    } else {
        for v in &video {
            stream_inf(
                &mut s,
                v,
                if has_audio {
                    audio_extra.copied()
                } else {
                    None
                },
            );
        }
    }
    s
}

/// Write one `#EXT-X-STREAM-INF` + its URI, optionally bound to an audio group.
fn stream_inf(s: &mut String, v: &Variant, audio: Option<&Variant>) {
    let mut codecs = v.stream.rfc6381();
    let mut bandwidth = v.stream.bitrate.unwrap_or(0);
    if let Some(a) = audio {
        codecs = format!("{codecs},{}", a.stream.rfc6381());
        bandwidth += a.stream.bitrate.unwrap_or(0);
    }

    let _ = write!(
        s,
        "#EXT-X-STREAM-INF:BANDWIDTH={bandwidth},CODECS=\"{codecs}\""
    );
    if let Some((w, h)) = v.stream.resolution {
        let _ = write!(s, ",RESOLUTION={w}x{h}");
    }
    if audio.is_some() {
        let _ = write!(s, ",AUDIO=\"{AUDIO_GROUP}\"");
    }
    let _ = writeln!(s, "\n{}", v.playlist_uri);
}

/// Build a VOD media playlist from an init segment and ordered segment refs.
pub fn media_playlist(init_uri: &str, segments: &[SegmentRef]) -> String {
    let target = segments
        .iter()
        .map(|s| s.duration)
        .fold(0.0_f64, f64::max)
        .ceil() as u64;
    let mut s = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n");
    let _ = writeln!(s, "#EXT-X-TARGETDURATION:{}", target);
    let _ = writeln!(s, "#EXT-X-MAP:URI=\"{}\"", init_uri);
    for seg in segments {
        let _ = writeln!(s, "#EXTINF:{:.3},\n{}", seg.duration, seg.uri);
    }
    s.push_str("#EXT-X-ENDLIST\n");
    s
}
