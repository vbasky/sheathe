//! HLS (RFC 8216 / Apple HLS) playlist generation for **sheathe**.
//!
//! Emits master + media playlists for VOD, EVENT, and live (sliding-window)
//! presentations, plus I-frame (trick-play) playlists, low-latency HLS parts
//! (`EXT-X-PART` / preload hints / server control), and `EXT-X-DATERANGE`
//! markers for SCTE-35. Output is differential-tested against Shaka Packager
//! on the VOD path; live/LL tags follow the HLS authoring specification.

use sheathe_core::{MediaKind, StreamInfo};
use std::fmt::Write as _;

/// One variant stream listed in the master playlist.
#[derive(Debug, Clone)]
pub struct Variant {
    /// The stream this variant carries.
    pub stream: StreamInfo,
    /// Path to this variant's media playlist, relative to the master.
    pub playlist_uri: String,
    /// Optional I-frame (trick-play) playlist URI for video variants.
    pub iframe_playlist_uri: Option<String>,
}

/// HLS playlist type (`#EXT-X-PLAYLIST-TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaylistType {
    /// Finished asset; includes `#EXT-X-ENDLIST`.
    #[default]
    Vod,
    /// Growing event; no ENDLIST until finalised.
    Event,
    /// Sliding-window live; no PLAYLIST-TYPE, no ENDLIST.
    Live,
}

/// A low-latency partial segment (`#EXT-X-PART`).
#[derive(Debug, Clone)]
pub struct PartialSegment {
    /// Part URI.
    pub uri: String,
    /// Part duration in seconds.
    pub duration: f64,
    /// Whether the part begins with an independent (keyframe) access unit.
    pub independent: bool,
}

/// A single media segment in a media playlist.
#[derive(Debug, Clone)]
pub struct SegmentRef {
    /// Segment duration in seconds (`#EXTINF`).
    pub duration: f64,
    /// Segment URI.
    pub uri: String,
    /// Optional `#EXT-X-PROGRAM-DATE-TIME` for this segment.
    pub program_date_time: Option<String>,
    /// Low-latency parts that make up this segment (empty for regular HLS).
    pub parts: Vec<PartialSegment>,
}

impl SegmentRef {
    /// Simple segment without parts or PDT.
    pub fn new(duration: f64, uri: impl Into<String>) -> Self {
        Self { duration, uri: uri.into(), program_date_time: None, parts: Vec::new() }
    }
}

/// HLS audio rendition group id used to bind video variants to audio.
const AUDIO_GROUP: &str = "aud";

/// CENC key signalling for an HLS media playlist (`#EXT-X-KEY`).
#[derive(Debug, Clone)]
pub struct KeyInfo {
    /// `SAMPLE-AES` (cbcs) or `SAMPLE-AES-CTR` (cenc).
    pub method: String,
    /// `KEYFORMAT` value, e.g. `urn:mpeg:dash:mp4protection:2011`.
    pub key_format: String,
    /// Key-delivery URI.
    pub uri: String,
}

/// An `#EXT-X-DATERANGE` marker (SCTE-35 ad cues, interstitials, …).
#[derive(Debug, Clone)]
pub struct DateRange {
    /// Unique id within the playlist.
    pub id: String,
    /// Optional class (e.g. `com.apple.hls.scte35`).
    pub class: Option<String>,
    /// ISO-8601 start date.
    pub start_date: String,
    /// Optional ISO-8601 end date.
    pub end_date: Option<String>,
    /// Optional duration in seconds.
    pub duration: Option<f64>,
    /// Optional planned duration in seconds.
    pub planned_duration: Option<f64>,
    /// Hex-encoded SCTE-35 splice_info_section for a cue-out.
    pub scte35_out: Option<String>,
    /// Hex-encoded SCTE-35 splice_info_section for a cue-in.
    pub scte35_in: Option<String>,
    /// Hex-encoded SCTE-35 command (generic).
    pub scte35_cmd: Option<String>,
    /// `END-ON-NEXT=YES`.
    pub end_on_next: bool,
}

impl DateRange {
    /// SCTE-35 cue-out marker.
    pub fn scte35_out(
        id: impl Into<String>,
        start_date: impl Into<String>,
        splice_hex: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            class: Some("com.apple.hls.scte35".into()),
            start_date: start_date.into(),
            end_date: None,
            duration: None,
            planned_duration: None,
            scte35_out: Some(splice_hex.into()),
            scte35_in: None,
            scte35_cmd: None,
            end_on_next: false,
        }
    }

    /// SCTE-35 cue-in marker.
    pub fn scte35_in(
        id: impl Into<String>,
        start_date: impl Into<String>,
        splice_hex: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            class: Some("com.apple.hls.scte35".into()),
            start_date: start_date.into(),
            end_date: None,
            duration: None,
            planned_duration: None,
            scte35_out: None,
            scte35_in: Some(splice_hex.into()),
            scte35_cmd: None,
            end_on_next: false,
        }
    }
}

/// Full media playlist configuration (VOD / EVENT / live / LL-HLS).
#[derive(Debug, Clone)]
pub struct MediaPlaylist {
    /// VOD / EVENT / Live.
    pub playlist_type: PlaylistType,
    /// Target duration (ceil of longest segment/part), or `None` to compute.
    pub target_duration: Option<u64>,
    /// `#EXT-X-MEDIA-SEQUENCE` (first segment number in the window).
    pub media_sequence: u64,
    /// Init segment URI (`#EXT-X-MAP`); required for fMP4/CMAF.
    pub init_uri: Option<String>,
    /// Optional encryption key.
    pub key: Option<KeyInfo>,
    /// Segments in the current window (or full asset for VOD/EVENT).
    pub segments: Vec<SegmentRef>,
    /// `#EXT-X-DATERANGE` markers.
    pub dateranges: Vec<DateRange>,
    /// LL-HLS part target duration (`#EXT-X-PART-INF:PART-TARGET=`).
    pub part_target: Option<f64>,
    /// LL-HLS hold-back for parts (`CAN-BLOCK-RELOAD` server control).
    pub part_hold_back: Option<f64>,
    /// `#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES`.
    pub can_block_reload: bool,
    /// Optional preload hint `(TYPE, URI)` — e.g. `("PART", "seg_1.3.m4s")`.
    pub preload_hint: Option<(String, String)>,
    /// Emit `#EXT-X-ENDLIST` (VOD always; EVENT when finalised).
    pub endlist: bool,
}

impl Default for MediaPlaylist {
    fn default() -> Self {
        Self {
            playlist_type: PlaylistType::Vod,
            target_duration: None,
            media_sequence: 0,
            init_uri: None,
            key: None,
            segments: Vec::new(),
            dateranges: Vec::new(),
            part_target: None,
            part_hold_back: None,
            can_block_reload: false,
            preload_hint: None,
            endlist: true,
        }
    }
}

impl MediaPlaylist {
    /// Classic VOD media playlist (init + segments + ENDLIST).
    pub fn vod(
        init_uri: impl Into<String>,
        segments: Vec<SegmentRef>,
        key: Option<KeyInfo>,
    ) -> Self {
        Self {
            playlist_type: PlaylistType::Vod,
            init_uri: Some(init_uri.into()),
            segments,
            key,
            endlist: true,
            ..Self::default()
        }
    }

    /// EVENT playlist: growing, no ENDLIST until the caller sets `endlist`.
    pub fn event(
        init_uri: impl Into<String>,
        segments: Vec<SegmentRef>,
        key: Option<KeyInfo>,
        endlist: bool,
    ) -> Self {
        Self {
            playlist_type: PlaylistType::Event,
            init_uri: Some(init_uri.into()),
            segments,
            key,
            endlist,
            ..Self::default()
        }
    }

    /// Live sliding-window playlist.
    pub fn live(
        init_uri: impl Into<String>,
        media_sequence: u64,
        segments: Vec<SegmentRef>,
        key: Option<KeyInfo>,
    ) -> Self {
        Self {
            playlist_type: PlaylistType::Live,
            media_sequence,
            init_uri: Some(init_uri.into()),
            segments,
            key,
            endlist: false,
            ..Self::default()
        }
    }

    /// Serialize to an M3U8 media playlist string.
    pub fn to_m3u8(&self) -> String {
        let target = self.target_duration.unwrap_or_else(|| compute_target_duration(self));
        let mut s = String::from("#EXTM3U\n#EXT-X-VERSION:7\n");

        match self.playlist_type {
            PlaylistType::Vod => s.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n"),
            PlaylistType::Event => s.push_str("#EXT-X-PLAYLIST-TYPE:EVENT\n"),
            PlaylistType::Live => {}
        }

        let _ = writeln!(s, "#EXT-X-TARGETDURATION:{target}");
        let _ = writeln!(s, "#EXT-X-MEDIA-SEQUENCE:{}", self.media_sequence);

        if self.can_block_reload || self.part_hold_back.is_some() {
            s.push_str("#EXT-X-SERVER-CONTROL:");
            let mut first = true;
            if self.can_block_reload {
                s.push_str("CAN-BLOCK-RELOAD=YES");
                first = false;
            }
            if let Some(phb) = self.part_hold_back {
                if !first {
                    s.push(',');
                }
                let _ = write!(s, "PART-HOLD-BACK={phb:.3}");
            }
            s.push('\n');
        }

        if let Some(pt) = self.part_target {
            let _ = writeln!(s, "#EXT-X-PART-INF:PART-TARGET={pt:.3}");
        }

        if let Some(k) = &self.key {
            let _ = writeln!(
                s,
                "#EXT-X-KEY:METHOD={},URI=\"{}\",KEYFORMAT=\"{}\",KEYFORMATVERSIONS=\"1\"",
                k.method, k.uri, k.key_format
            );
        }

        if let Some(init) = &self.init_uri {
            let _ = writeln!(s, "#EXT-X-MAP:URI=\"{init}\"");
        }

        for dr in &self.dateranges {
            render_daterange(&mut s, dr);
        }

        for seg in &self.segments {
            if let Some(pdt) = &seg.program_date_time {
                let _ = writeln!(s, "#EXT-X-PROGRAM-DATE-TIME:{pdt}");
            }
            for part in &seg.parts {
                let _ = write!(s, "#EXT-X-PART:DURATION={:.3},URI=\"{}\"", part.duration, part.uri);
                if part.independent {
                    s.push_str(",INDEPENDENT=YES");
                }
                s.push('\n');
            }
            let _ = writeln!(s, "#EXTINF:{:.3},\n{}", seg.duration, seg.uri);
        }

        if let Some((ty, uri)) = &self.preload_hint {
            let _ = writeln!(s, "#EXT-X-PRELOAD-HINT:TYPE={ty},URI=\"{uri}\"");
        }

        if self.endlist {
            s.push_str("#EXT-X-ENDLIST\n");
        }
        s
    }
}

fn compute_target_duration(p: &MediaPlaylist) -> u64 {
    let mut max = 0.0_f64;
    for seg in &p.segments {
        max = max.max(seg.duration);
        for part in &seg.parts {
            max = max.max(part.duration);
        }
    }
    if let Some(pt) = p.part_target {
        max = max.max(pt);
    }
    max.ceil().max(1.0) as u64
}

fn render_daterange(s: &mut String, dr: &DateRange) {
    let _ = write!(s, "#EXT-X-DATERANGE:ID=\"{}\",START-DATE=\"{}\"", dr.id, dr.start_date);
    if let Some(c) = &dr.class {
        let _ = write!(s, ",CLASS=\"{c}\"");
    }
    if let Some(e) = &dr.end_date {
        let _ = write!(s, ",END-DATE=\"{e}\"");
    }
    if let Some(d) = dr.duration {
        let _ = write!(s, ",DURATION={d:.3}");
    }
    if let Some(d) = dr.planned_duration {
        let _ = write!(s, ",PLANNED-DURATION={d:.3}");
    }
    if let Some(h) = &dr.scte35_out {
        let _ = write!(s, ",SCTE35-OUT={h}");
    }
    if let Some(h) = &dr.scte35_in {
        let _ = write!(s, ",SCTE35-IN={h}");
    }
    if let Some(h) = &dr.scte35_cmd {
        let _ = write!(s, ",SCTE35-CMD={h}");
    }
    if dr.end_on_next {
        s.push_str(",END-ON-NEXT=YES");
    }
    s.push('\n');
}

/// Build a master playlist over `variants`.
///
/// Audio variants are emitted as an `#EXT-X-MEDIA` rendition group; video
/// variants become `#EXT-X-STREAM-INF` entries that reference it (with the audio
/// codec folded into `CODECS` and bandwidth). With no video, audio variants fall
/// back to plain `#EXT-X-STREAM-INF` so the playlist is still usable.
///
/// Video variants with [`Variant::iframe_playlist_uri`] also emit
/// `#EXT-X-I-FRAME-STREAM-INF` for trick-play.
pub fn master_playlist(variants: &[Variant]) -> String {
    let audio: Vec<&Variant> =
        variants.iter().filter(|v| v.stream.kind == MediaKind::Audio).collect();
    let video: Vec<&Variant> =
        variants.iter().filter(|v| v.stream.kind == MediaKind::Video).collect();

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
        for a in &audio {
            stream_inf(&mut s, a, None);
        }
    } else {
        for v in &video {
            stream_inf(&mut s, v, if has_audio { audio_extra.copied() } else { None });
        }
        for v in &video {
            if let Some(iframe) = &v.iframe_playlist_uri {
                iframe_stream_inf(&mut s, v, iframe);
            }
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

    let _ = write!(s, "#EXT-X-STREAM-INF:BANDWIDTH={bandwidth},CODECS=\"{codecs}\"");
    if let Some((w, h)) = v.stream.resolution {
        let _ = write!(s, ",RESOLUTION={w}x{h}");
    }
    if audio.is_some() {
        let _ = write!(s, ",AUDIO=\"{AUDIO_GROUP}\"");
    }
    let _ = writeln!(s, "\n{}", v.playlist_uri);
}

/// Write one `#EXT-X-I-FRAME-STREAM-INF` for trick-play.
fn iframe_stream_inf(s: &mut String, v: &Variant, iframe_uri: &str) {
    let codecs = v.stream.rfc6381();
    // I-frame playlists are typically a fraction of the full bitrate.
    let bandwidth = v.stream.bitrate.unwrap_or(0) / 10;
    let _ = write!(
        s,
        "#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH={bandwidth},CODECS=\"{codecs}\",URI=\"{iframe_uri}\""
    );
    if let Some((w, h)) = v.stream.resolution {
        let _ = write!(s, ",RESOLUTION={w}x{h}");
    }
    s.push('\n');
}

/// Build a VOD media playlist from an init segment and ordered segment refs.
///
/// When `key` is set, an `#EXT-X-KEY` line signals CENC encryption to players.
///
/// Prefer [`MediaPlaylist::vod`] + [`MediaPlaylist::to_m3u8`] for new code; this
/// free function is the historical API and remains stable.
pub fn media_playlist(init_uri: &str, segments: &[SegmentRef], key: Option<&KeyInfo>) -> String {
    MediaPlaylist::vod(init_uri, segments.to_vec(), key.cloned()).to_m3u8()
}

/// Build an I-frame-only (trick-play) media playlist.
///
/// Segments are expected to contain only IDR/keyframe samples. Emits
/// `#EXT-X-I-FRAMES-ONLY` and the usual MAP/INF structure.
pub fn iframe_playlist(init_uri: &str, segments: &[SegmentRef]) -> String {
    let target = segments.iter().map(|s| s.duration).fold(0.0_f64, f64::max).ceil().max(1.0) as u64;
    let mut s = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n");
    let _ = writeln!(s, "#EXT-X-TARGETDURATION:{target}");
    s.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    s.push_str("#EXT-X-I-FRAMES-ONLY\n");
    let _ = writeln!(s, "#EXT-X-MAP:URI=\"{init_uri}\"");
    for seg in segments {
        let _ = writeln!(s, "#EXTINF:{:.3},\n{}", seg.duration, seg.uri);
    }
    s.push_str("#EXT-X-ENDLIST\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheathe_core::{Codec, Timescale};

    fn video() -> StreamInfo {
        StreamInfo {
            kind: MediaKind::Video,
            codec: Codec::H264,
            timescale: Timescale(90000),
            resolution: Some((1280, 720)),
            sample_rate: None,
            bitrate: Some(2_500_000),
            codec_string: Some("avc1.64001f".into()),
        }
    }

    fn audio() -> StreamInfo {
        StreamInfo {
            kind: MediaKind::Audio,
            codec: Codec::Aac,
            timescale: Timescale(48000),
            resolution: None,
            sample_rate: Some(48000),
            bitrate: Some(128_000),
            codec_string: Some("mp4a.40.2".into()),
        }
    }

    #[test]
    fn vod_media_playlist_shape() {
        let segs = vec![SegmentRef::new(6.0, "seg_0_1.m4s"), SegmentRef::new(4.0, "seg_0_2.m4s")];
        let pl = media_playlist("init_0.mp4", &segs, None);
        assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(pl.contains("#EXT-X-TARGETDURATION:6"));
        assert!(pl.contains("#EXT-X-MAP:URI=\"init_0.mp4\""));
        assert!(pl.contains("#EXT-X-ENDLIST"));
        assert!(pl.contains("#EXT-X-MEDIA-SEQUENCE:0"));
    }

    #[test]
    fn live_sliding_window_no_endlist() {
        let segs = vec![
            SegmentRef::new(6.0, "seg_0_8.m4s"),
            SegmentRef::new(6.0, "seg_0_9.m4s"),
            SegmentRef::new(6.0, "seg_0_10.m4s"),
        ];
        let pl = MediaPlaylist::live("init_0.mp4", 7, segs, None).to_m3u8();
        assert!(!pl.contains("PLAYLIST-TYPE"));
        assert!(pl.contains("#EXT-X-MEDIA-SEQUENCE:7"));
        assert!(!pl.contains("#EXT-X-ENDLIST"));
        assert!(pl.contains("seg_0_10.m4s"));
    }

    #[test]
    fn event_playlist_type() {
        let segs = vec![SegmentRef::new(6.0, "seg_0_1.m4s")];
        let pl = MediaPlaylist::event("init_0.mp4", segs, None, false).to_m3u8();
        assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
        assert!(!pl.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn ll_hls_parts_and_server_control() {
        let mut seg = SegmentRef::new(6.0, "seg_0_1.m4s");
        seg.parts = vec![
            PartialSegment { uri: "seg_0_1.1.m4s".into(), duration: 2.0, independent: true },
            PartialSegment { uri: "seg_0_1.2.m4s".into(), duration: 2.0, independent: false },
            PartialSegment { uri: "seg_0_1.3.m4s".into(), duration: 2.0, independent: false },
        ];
        let mut pl = MediaPlaylist::live("init_0.mp4", 0, vec![seg], None);
        pl.part_target = Some(2.0);
        pl.part_hold_back = Some(6.0);
        pl.can_block_reload = true;
        pl.preload_hint = Some(("PART".into(), "seg_0_2.1.m4s".into()));
        let out = pl.to_m3u8();
        assert!(out.contains("#EXT-X-PART-INF:PART-TARGET=2.000"));
        assert!(out.contains("CAN-BLOCK-RELOAD=YES"));
        assert!(out.contains("PART-HOLD-BACK=6.000"));
        assert!(out.contains("#EXT-X-PART:DURATION=2.000,URI=\"seg_0_1.1.m4s\",INDEPENDENT=YES"));
        assert!(out.contains("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"seg_0_2.1.m4s\""));
    }

    #[test]
    fn daterange_scte35() {
        let segs = vec![SegmentRef::new(6.0, "seg_0_1.m4s")];
        let mut pl = MediaPlaylist::vod("init_0.mp4", segs, None);
        pl.dateranges.push(DateRange::scte35_out(
            "ad1",
            "2026-07-18T00:00:06Z",
            "0xFC30200000000000000000000000600006456E0000000000",
        ));
        let out = pl.to_m3u8();
        assert!(out.contains("#EXT-X-DATERANGE:ID=\"ad1\""));
        assert!(out.contains("SCTE35-OUT=0xFC"));
        assert!(out.contains("CLASS=\"com.apple.hls.scte35\""));
    }

    #[test]
    fn iframe_playlist_and_master() {
        let segs = vec![SegmentRef::new(2.0, "seg_0_trick_1.m4s")];
        let iframe = iframe_playlist("init_0_trick.mp4", &segs);
        assert!(iframe.contains("#EXT-X-I-FRAMES-ONLY"));
        assert!(iframe.contains("#EXT-X-ENDLIST"));

        let variants = vec![
            Variant {
                stream: video(),
                playlist_uri: "media_0.m3u8".into(),
                iframe_playlist_uri: Some("iframe_0.m3u8".into()),
            },
            Variant {
                stream: audio(),
                playlist_uri: "media_1.m3u8".into(),
                iframe_playlist_uri: None,
            },
        ];
        let master = master_playlist(&variants);
        assert!(master.contains("#EXT-X-STREAM-INF:"));
        assert!(master.contains("#EXT-X-I-FRAME-STREAM-INF:"));
        assert!(master.contains("URI=\"iframe_0.m3u8\""));
        assert!(master.contains("AUDIO=\"aud\""));
    }
}
