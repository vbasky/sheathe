//! MPEG-DASH (ISO/IEC 23009-1) manifest generation for **sheathe**.
//!
//! Emits static (on-demand) and dynamic (live) MPDs using `SegmentTemplate` +
//! `SegmentTimeline`. Supports multi-period presentations, trick-play
//! AdaptationSets, low-latency `availabilityTimeOffset`, and SCTE-35
//! `EventStream` markers. Output is differential-tested against Shaka Packager
//! on the VOD path; live/LL fields follow the DASH-IF live profile conventions.

use sheathe_core::{MediaKind, StreamInfo};
use std::fmt::Write as _;

/// How a representation addresses its segments in the MPD.
#[derive(Debug, Clone)]
pub enum SegmentAddressing {
    /// Live/VOD multi-file layout: `SegmentTemplate` + `SegmentTimeline`.
    Template {
        /// Initialization segment URL.
        init: String,
        /// Media segment URL template (may contain `$Number$`).
        media: String,
        /// First segment number (default 1).
        start_number: u32,
        /// Presentation time offset of the first timeline sample (timescale ticks).
        presentation_time_offset: u64,
        /// Low-latency availability time offset in seconds.
        availability_time_offset: Option<f64>,
    },
    /// On-demand single-file layout: `SegmentBase` with byte ranges.
    ///
    /// File layout is `[init][sidx?][media…]` or `[init][seg1][seg2]…` with
    /// explicit media ranges in [`SegmentBaseInfo::media_ranges`].
    Base(SegmentBaseInfo),
}

/// Byte-range addressing for a single-file on-demand representation.
#[derive(Debug, Clone)]
pub struct SegmentBaseInfo {
    /// Relative URL of the single media file (`BaseURL`).
    pub base_url: String,
    /// Inclusive byte range of the initialization segment (`a-b`).
    pub init_range: (u64, u64),
    /// Inclusive byte range of the segment index (`sidx`), if present.
    pub index_range: Option<(u64, u64)>,
    /// Inclusive byte ranges of each media segment, in order.
    /// When non-empty, emitted as `SegmentList`/`SegmentURL@mediaRange`
    /// (more precise than a single index for multi-fragment files).
    pub media_ranges: Vec<(u64, u64)>,
}

/// One selectable rendition within an adaptation set.
#[derive(Debug, Clone)]
pub struct Representation {
    /// A unique id within the manifest (also used in segment URLs).
    pub id: String,
    /// The stream this representation carries.
    pub stream: StreamInfo,
    /// Timescale the segment durations are expressed in.
    pub timescale: u32,
    /// Per-segment durations, in `timescale` ticks, in order.
    pub segment_durations: Vec<u64>,
    /// How segments are addressed (template vs on-demand base).
    pub addressing: SegmentAddressing,
    /// When set, this is a trick-play representation (`maxPlayoutRate`).
    pub max_playout_rate: Option<f64>,
}

impl Representation {
    /// Build a standard multi-file VOD representation starting at segment number 1.
    pub fn new(
        id: impl Into<String>,
        stream: StreamInfo,
        init: impl Into<String>,
        media: impl Into<String>,
        timescale: u32,
        segment_durations: Vec<u64>,
    ) -> Self {
        Self {
            id: id.into(),
            stream,
            timescale,
            segment_durations,
            addressing: SegmentAddressing::Template {
                init: init.into(),
                media: media.into(),
                start_number: 1,
                presentation_time_offset: 0,
                availability_time_offset: None,
            },
            max_playout_rate: None,
        }
    }

    /// Build an on-demand single-file representation with byte-range addressing.
    pub fn on_demand(
        id: impl Into<String>,
        stream: StreamInfo,
        timescale: u32,
        segment_durations: Vec<u64>,
        base: SegmentBaseInfo,
    ) -> Self {
        Self {
            id: id.into(),
            stream,
            timescale,
            segment_durations,
            addressing: SegmentAddressing::Base(base),
            max_playout_rate: None,
        }
    }

    /// Convenience accessors for template fields (legacy package path).
    pub fn init(&self) -> Option<&str> {
        match &self.addressing {
            SegmentAddressing::Template { init, .. } => Some(init.as_str()),
            SegmentAddressing::Base(_) => None,
        }
    }

    /// Mutable template start number (no-op for on-demand).
    pub fn set_start_number(&mut self, n: u32) {
        if let SegmentAddressing::Template { start_number, .. } = &mut self.addressing {
            *start_number = n;
        }
    }

    /// Mutable template presentation time offset (no-op for on-demand).
    pub fn set_presentation_time_offset(&mut self, pto: u64) {
        if let SegmentAddressing::Template { presentation_time_offset, .. } = &mut self.addressing {
            *presentation_time_offset = pto;
        }
    }

    /// Mutable template availability time offset (no-op for on-demand).
    pub fn set_availability_time_offset(&mut self, ato: Option<f64>) {
        if let SegmentAddressing::Template { availability_time_offset, .. } = &mut self.addressing {
            *availability_time_offset = ato;
        }
    }
}

/// CENC protection signalling for the manifest (`ContentProtection`).
#[derive(Debug, Clone)]
pub struct Protection {
    /// Scheme value: `cenc`, `cbcs`, `cbc1`, or `cens`.
    pub scheme: String,
    /// 16-byte default Key ID, rendered as a dashed UUID.
    pub default_kid: [u8; 16],
}

/// MPD `@type`: static (VOD) or dynamic (live).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MpdType {
    /// Finished presentation with known duration.
    #[default]
    Static,
    /// Live or event presentation that may still grow.
    Dynamic,
}

/// `UTCTiming` element for wall-clock synchronisation on dynamic MPDs.
#[derive(Debug, Clone)]
pub struct UtcTiming {
    /// Scheme URI (e.g. `urn:mpeg:dash:utc:http-iso:2014`).
    pub scheme_id_uri: String,
    /// Scheme-specific value (URL for HTTP schemes; may be empty).
    pub value: String,
}

impl UtcTiming {
    /// Common HTTP-ISO wall-clock source.
    pub fn http_iso(url: impl Into<String>) -> Self {
        Self { scheme_id_uri: "urn:mpeg:dash:utc:http-iso:2014".into(), value: url.into() }
    }

    /// Direct wall-clock (no network fetch); value is an ISO-8601 timestamp.
    pub fn direct(iso_time: impl Into<String>) -> Self {
        Self { scheme_id_uri: "urn:mpeg:dash:utc:direct:2014".into(), value: iso_time.into() }
    }
}

/// One timed event inside an [`EventStream`] (e.g. a SCTE-35 splice).
#[derive(Debug, Clone)]
pub struct DashEvent {
    /// Optional event id.
    pub id: Option<String>,
    /// Presentation time in the event stream's timescale.
    pub presentation_time: u64,
    /// Optional duration in the event stream's timescale.
    pub duration: Option<u64>,
    /// Optional message body (often base64-encoded SCTE-35 binary).
    pub message_data: Option<String>,
}

/// DASH `EventStream` (SCTE-35 ad markers, interstitials, …).
#[derive(Debug, Clone)]
pub struct EventStream {
    /// Scheme URI — for binary SCTE-35: `urn:scte:scte35:2014:xml+bin`.
    pub scheme_id_uri: String,
    /// Optional scheme value.
    pub value: Option<String>,
    /// Timescale for event presentation times / durations.
    pub timescale: u32,
    /// Events in presentation order.
    pub events: Vec<DashEvent>,
}

impl EventStream {
    /// SCTE-35 binary event stream (`urn:scte:scte35:2014:xml+bin`).
    pub fn scte35_bin(timescale: u32, events: Vec<DashEvent>) -> Self {
        Self {
            scheme_id_uri: "urn:scte:scte35:2014:xml+bin".into(),
            value: None,
            timescale,
            events,
        }
    }
}

/// One contiguous period of content within a presentation.
#[derive(Debug, Clone)]
pub struct Period {
    /// Period id (unique within the MPD).
    pub id: String,
    /// Start offset from the period origin in seconds (`Period@start`).
    pub start_seconds: Option<f64>,
    /// Period duration in seconds when known (`Period@duration`).
    pub duration_seconds: Option<f64>,
    /// Representations grouped into AdaptationSets by media kind at render time.
    pub representations: Vec<Representation>,
    /// Period-level event streams (SCTE-35, …).
    pub event_streams: Vec<EventStream>,
}

impl Period {
    /// Single-period VOD helper: id `"0"`, start at 0, optional duration.
    pub fn single(
        duration_seconds: Option<f64>,
        representations: Vec<Representation>,
        event_streams: Vec<EventStream>,
    ) -> Self {
        Self {
            id: "0".into(),
            start_seconds: Some(0.0),
            duration_seconds,
            representations,
            event_streams,
        }
    }
}

/// DASH profile family for the `@profiles` attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashProfile {
    /// Multi-segment live/VOD with `SegmentTemplate` (`isoff-live`).
    #[default]
    Live,
    /// Single-file on-demand with `SegmentBase`/`SegmentList` (`isoff-on-demand`).
    OnDemand,
}

/// A complete DASH presentation (static or dynamic).
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Static vs dynamic MPD.
    pub mpd_type: MpdType,
    /// Profile family (live template vs on-demand base).
    pub profile: DashProfile,
    /// Total media duration (static only); omitted on pure live.
    pub duration_seconds: Option<f64>,
    /// Wall-clock origin for dynamic timelines (`@availabilityStartTime`).
    pub availability_start_time: Option<String>,
    /// When this MPD was published (`@publishTime`).
    pub publish_time: Option<String>,
    /// Client refresh interval for dynamic MPDs (`@minimumUpdatePeriod`).
    pub minimum_update_period: Option<f64>,
    /// Live edge buffer depth (`@timeShiftBufferDepth`).
    pub time_shift_buffer_depth: Option<f64>,
    /// Suggested delay behind the live edge (`@suggestedPresentationDelay`).
    pub suggested_presentation_delay: Option<f64>,
    /// Wall-clock timing source for dynamic MPDs.
    pub utc_timing: Option<UtcTiming>,
    /// One or more periods.
    pub periods: Vec<Period>,
    /// When set, emit `ContentProtection` elements (encrypted content).
    pub protection: Option<Protection>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            mpd_type: MpdType::Static,
            profile: DashProfile::Live,
            duration_seconds: None,
            availability_start_time: None,
            publish_time: None,
            minimum_update_period: None,
            time_shift_buffer_depth: None,
            suggested_presentation_delay: None,
            utc_timing: None,
            periods: Vec::new(),
            protection: None,
        }
    }
}

impl Manifest {
    /// Convenience constructor for a single-period static VOD presentation.
    ///
    /// This is the historical sheathe MPD shape and remains the default for the
    /// VOD package path.
    pub fn static_vod(
        duration_seconds: f64,
        representations: Vec<Representation>,
        protection: Option<Protection>,
    ) -> Self {
        Self {
            mpd_type: MpdType::Static,
            duration_seconds: Some(duration_seconds),
            periods: vec![Period::single(Some(duration_seconds), representations, Vec::new())],
            protection,
            ..Self::default()
        }
    }

    /// Convenience constructor for a single-period dynamic (live) presentation.
    pub fn dynamic_live(
        availability_start_time: impl Into<String>,
        publish_time: impl Into<String>,
        time_shift_buffer_depth: f64,
        minimum_update_period: f64,
        suggested_presentation_delay: f64,
        representations: Vec<Representation>,
        protection: Option<Protection>,
    ) -> Self {
        Self {
            mpd_type: MpdType::Dynamic,
            availability_start_time: Some(availability_start_time.into()),
            publish_time: Some(publish_time.into()),
            minimum_update_period: Some(minimum_update_period),
            time_shift_buffer_depth: Some(time_shift_buffer_depth),
            suggested_presentation_delay: Some(suggested_presentation_delay),
            utc_timing: Some(UtcTiming::http_iso("https://time.akamai.com/?iso")),
            periods: vec![Period::single(None, representations, Vec::new())],
            protection,
            ..Self::default()
        }
    }

    /// Serialize to an MPD XML string.
    pub fn to_xml(&self) -> String {
        let mut s = String::new();
        s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let cenc_ns =
            if self.protection.is_some() { " xmlns:cenc=\"urn:mpeg:cenc:2013\"" } else { "" };
        let type_str = match self.mpd_type {
            MpdType::Static => "static",
            MpdType::Dynamic => "dynamic",
        };
        let profile_uri = match self.profile {
            DashProfile::Live => "urn:mpeg:dash:profile:isoff-live:2011",
            DashProfile::OnDemand => "urn:mpeg:dash:profile:isoff-on-demand:2011",
        };
        let _ = write!(
            s,
            concat!(
                "<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\"{} ",
                "profiles=\"{}\" ",
                "type=\"{}\" minBufferTime=\"PT2S\""
            ),
            cenc_ns, profile_uri, type_str,
        );
        if let Some(d) = self.duration_seconds {
            let _ = write!(s, " mediaPresentationDuration=\"{}\"", iso8601_duration(d));
        }
        if let Some(t) = &self.availability_start_time {
            let _ = write!(s, " availabilityStartTime=\"{}\"", xml_escape(t));
        }
        if let Some(t) = &self.publish_time {
            let _ = write!(s, " publishTime=\"{}\"", xml_escape(t));
        }
        if let Some(p) = self.minimum_update_period {
            let _ = write!(s, " minimumUpdatePeriod=\"{}\"", iso8601_duration(p));
        }
        if let Some(d) = self.time_shift_buffer_depth {
            let _ = write!(s, " timeShiftBufferDepth=\"{}\"", iso8601_duration(d));
        }
        if let Some(d) = self.suggested_presentation_delay {
            let _ = write!(s, " suggestedPresentationDelay=\"{}\"", iso8601_duration(d));
        }
        s.push_str(">\n");

        if let Some(utc) = &self.utc_timing {
            let _ = writeln!(
                s,
                "  <UTCTiming schemeIdUri=\"{}\" value=\"{}\"/>",
                xml_escape(&utc.scheme_id_uri),
                xml_escape(&utc.value),
            );
        }

        for period in &self.periods {
            render_period(&mut s, period, self.protection.as_ref());
        }

        s.push_str("</MPD>\n");
        s
    }
}

fn render_period(s: &mut String, period: &Period, protection: Option<&Protection>) {
    let _ = write!(s, "  <Period id=\"{}\"", xml_escape(&period.id));
    if let Some(start) = period.start_seconds {
        let _ = write!(s, " start=\"{}\"", iso8601_duration(start));
    }
    if let Some(dur) = period.duration_seconds {
        let _ = write!(s, " duration=\"{}\"", iso8601_duration(dur));
    }
    s.push_str(">\n");

    for es in &period.event_streams {
        render_event_stream(s, es);
    }

    // Regular AdaptationSets by kind, then a separate trick-play set for video
    // representations that carry maxPlayoutRate.
    for (kind, content_type) in
        [(MediaKind::Video, "video"), (MediaKind::Audio, "audio"), (MediaKind::Text, "text")]
    {
        let normal: Vec<&Representation> = period
            .representations
            .iter()
            .filter(|r| r.stream.kind == kind && r.max_playout_rate.is_none())
            .collect();
        if !normal.is_empty() {
            render_adaptation_set(s, content_type, &normal, protection, false);
        }

        if kind == MediaKind::Video {
            let trick: Vec<&Representation> = period
                .representations
                .iter()
                .filter(|r| r.stream.kind == MediaKind::Video && r.max_playout_rate.is_some())
                .collect();
            if !trick.is_empty() {
                render_adaptation_set(s, "video", &trick, protection, true);
            }
        }
    }

    s.push_str("  </Period>\n");
}

fn render_adaptation_set(
    s: &mut String,
    content_type: &str,
    reps: &[&Representation],
    protection: Option<&Protection>,
    trick_play: bool,
) {
    let _ = writeln!(
        s,
        "    <AdaptationSet contentType=\"{}\" segmentAlignment=\"true\">",
        content_type
    );
    if trick_play {
        // DASH-IF IOP: trick-mode AdaptationSet is marked with EssentialProperty
        // referencing the main content AdaptationSet id. We use value="1" as a
        // stable placeholder when AdaptationSet ids are not assigned.
        s.push_str(
            "      <EssentialProperty schemeIdUri=\"http://dashif.org/guidelines/trickmode\" value=\"1\"/>\n",
        );
    }
    if let Some(p) = protection {
        render_content_protection(s, p);
    }
    for r in reps {
        render_representation(s, r);
    }
    s.push_str("    </AdaptationSet>\n");
}

fn render_content_protection(s: &mut String, p: &Protection) {
    let _ = writeln!(
        s,
        concat!(
            "      <ContentProtection ",
            "schemeIdUri=\"urn:mpeg:dash:mp4protection:2011\" ",
            "value=\"{}\" cenc:default_KID=\"{}\"/>"
        ),
        p.scheme,
        kid_uuid(&p.default_kid),
    );
}

fn render_event_stream(s: &mut String, es: &EventStream) {
    let _ = write!(
        s,
        "    <EventStream schemeIdUri=\"{}\" timescale=\"{}\"",
        xml_escape(&es.scheme_id_uri),
        es.timescale
    );
    if let Some(v) = &es.value {
        let _ = write!(s, " value=\"{}\"", xml_escape(v));
    }
    s.push_str(">\n");
    for ev in &es.events {
        let _ = write!(s, "      <Event presentationTime=\"{}\"", ev.presentation_time);
        if let Some(d) = ev.duration {
            let _ = write!(s, " duration=\"{d}\"");
        }
        if let Some(id) = &ev.id {
            let _ = write!(s, " id=\"{}\"", xml_escape(id));
        }
        if let Some(msg) = &ev.message_data {
            let _ = writeln!(s, ">{}</Event>", xml_escape(msg));
        } else {
            s.push_str("/>\n");
        }
    }
    s.push_str("    </EventStream>\n");
}

/// Format a 16-byte KID as a dashed UUID (8-4-4-4-12).
fn kid_uuid(kid: &[u8; 16]) -> String {
    let h: String = kid.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn render_representation(s: &mut String, r: &Representation) {
    let codec = r.stream.rfc6381();
    let bandwidth = r.stream.bitrate.unwrap_or(0);
    let _ = write!(s, "      <Representation id=\"{}\" codecs=\"{}\"", r.id, codec);
    if let Some((w, h)) = r.stream.resolution {
        let _ = write!(s, " width=\"{}\" height=\"{}\"", w, h);
    }
    if let Some(rate) = r.stream.sample_rate {
        let _ = write!(s, " audioSamplingRate=\"{}\"", rate);
    }
    if let Some(mpr) = r.max_playout_rate {
        let _ = write!(s, " maxPlayoutRate=\"{mpr}\"");
    }
    let _ = writeln!(s, " bandwidth=\"{}\">", bandwidth);

    match &r.addressing {
        SegmentAddressing::Template {
            init,
            media,
            start_number,
            presentation_time_offset,
            availability_time_offset,
        } => {
            let _ = write!(
                s,
                "        <SegmentTemplate timescale=\"{}\" initialization=\"{}\" media=\"{}\" startNumber=\"{}\"",
                r.timescale, init, media, start_number
            );
            if *presentation_time_offset > 0 {
                let _ = write!(s, " presentationTimeOffset=\"{presentation_time_offset}\"");
            }
            if let Some(ato) = availability_time_offset {
                let _ = write!(
                    s,
                    " availabilityTimeOffset=\"{ato}\" availabilityTimeComplete=\"false\""
                );
            }
            s.push_str(">\n");
            s.push_str("          <SegmentTimeline>\n");
            render_timeline(s, &r.segment_durations, *presentation_time_offset);
            s.push_str("          </SegmentTimeline>\n");
            s.push_str("        </SegmentTemplate>\n");
        }
        SegmentAddressing::Base(base) => {
            let _ = writeln!(s, "        <BaseURL>{}</BaseURL>", xml_escape(&base.base_url));
            if base.media_ranges.is_empty() {
                // Pure SegmentBase (init + optional index only).
                let _ = write!(s, "        <SegmentBase timescale=\"{}\"", r.timescale);
                if let Some((a, b)) = base.index_range {
                    let _ = write!(s, " indexRange=\"{a}-{b}\"");
                }
                s.push_str(">\n");
                let (ia, ib) = base.init_range;
                let _ = writeln!(s, "          <Initialization range=\"{ia}-{ib}\"/>");
                s.push_str("        </SegmentBase>\n");
            } else {
                // SegmentList with explicit media ranges — preferred for
                // multi-fragment single files.
                let _ = writeln!(s, "        <SegmentList timescale=\"{}\">", r.timescale);
                let (ia, ib) = base.init_range;
                let _ = writeln!(s, "          <Initialization range=\"{ia}-{ib}\"/>");
                for (a, b) in &base.media_ranges {
                    let _ = writeln!(s, "          <SegmentURL mediaRange=\"{a}-{b}\"/>");
                }
                s.push_str("        </SegmentList>\n");
            }
        }
    }
    s.push_str("      </Representation>\n");
}

/// Emit `<S>` entries, collapsing runs of equal durations with `r=`.
fn render_timeline(s: &mut String, durations: &[u64], start_t: u64) {
    let mut t = start_t;
    let mut i = 0;
    let mut first = true;
    while i < durations.len() {
        let d = durations[i];
        let mut run = 1;
        while i + run < durations.len() && durations[i + run] == d {
            run += 1;
        }
        s.push_str("            <S");
        if first {
            let _ = write!(s, " t=\"{t}\"");
            first = false;
        }
        let _ = write!(s, " d=\"{d}\"");
        if run > 1 {
            let _ = write!(s, " r=\"{}\"", run - 1);
        }
        s.push_str("/>\n");
        t += d * run as u64;
        i += run;
    }
}

/// Format seconds as an ISO 8601 duration (e.g. `PT1M30.500S`).
pub fn iso8601_duration(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round() as i64;
    let sign = if total_ms < 0 { "-" } else { "" };
    let total_ms = total_ms.unsigned_abs();
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let sec = (total_ms % 60_000) as f64 / 1000.0;
    let mut out = format!("{sign}PT");
    if h > 0 {
        let _ = write!(out, "{h}H");
    }
    if m > 0 {
        let _ = write!(out, "{m}M");
    }
    let _ = write!(out, "{sec}S");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheathe_core::{Codec, Timescale};

    fn video_stream() -> StreamInfo {
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

    fn audio_stream() -> StreamInfo {
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
    fn static_vod_mpd_shape() {
        let rep = Representation::new(
            "0",
            video_stream(),
            "init_0.mp4",
            "seg_0_$Number$.m4s",
            90000,
            vec![540_000, 540_000, 270_000],
        );
        let xml = Manifest::static_vod(15.0, vec![rep], None).to_xml();
        assert!(xml.contains("type=\"static\""));
        assert!(xml.contains("mediaPresentationDuration=\"PT15S\""));
        assert!(xml.contains("<Period id=\"0\""));
        assert!(xml.contains("startNumber=\"1\""));
        assert!(xml.contains("d=\"540000\" r=\"1\""));
        assert!(xml.contains("d=\"270000\""));
        assert!(!xml.contains("availabilityStartTime"));
    }

    #[test]
    fn dynamic_live_mpd_shape() {
        let mut rep = Representation::new(
            "0",
            video_stream(),
            "init_0.mp4",
            "seg_0_$Number$.m4s",
            90000,
            vec![540_000, 540_000, 540_000],
        );
        rep.set_start_number(10);
        rep.set_presentation_time_offset(9 * 540_000);
        let xml = Manifest::dynamic_live(
            "2026-07-18T00:00:00Z",
            "2026-07-18T00:01:00Z",
            30.0,
            2.0,
            10.0,
            vec![rep],
            None,
        )
        .to_xml();
        assert!(xml.contains("type=\"dynamic\""));
        assert!(xml.contains("availabilityStartTime=\"2026-07-18T00:00:00Z\""));
        assert!(xml.contains("publishTime=\"2026-07-18T00:01:00Z\""));
        assert!(xml.contains("minimumUpdatePeriod=\"PT2S\""));
        assert!(xml.contains("timeShiftBufferDepth=\"PT30S\""));
        assert!(xml.contains("suggestedPresentationDelay=\"PT10S\""));
        assert!(xml.contains("<UTCTiming"));
        assert!(xml.contains("startNumber=\"10\""));
        assert!(xml.contains("presentationTimeOffset=\"4860000\""));
        assert!(!xml.contains("mediaPresentationDuration"));
        assert!(!xml.contains("#EXT-X-ENDLIST")); // sanity: not HLS
    }

    #[test]
    fn multi_period_and_scte35_event_stream() {
        let v = Representation::new(
            "v0",
            video_stream(),
            "init_v0.mp4",
            "seg_v0_$Number$.m4s",
            90000,
            vec![900_000],
        );
        let a = Representation::new(
            "a0",
            audio_stream(),
            "init_a0.mp4",
            "seg_a0_$Number$.m4s",
            48000,
            vec![480_000],
        );
        let events = EventStream::scte35_bin(
            90000,
            vec![DashEvent {
                id: Some("1".into()),
                presentation_time: 0,
                duration: Some(900_000),
                message_data: Some("BASE64SCTE".into()),
            }],
        );
        let m = Manifest {
            mpd_type: MpdType::Static,
            duration_seconds: Some(20.0),
            periods: vec![
                Period {
                    id: "p0".into(),
                    start_seconds: Some(0.0),
                    duration_seconds: Some(10.0),
                    representations: vec![v.clone()],
                    event_streams: vec![events],
                },
                Period {
                    id: "p1".into(),
                    start_seconds: Some(10.0),
                    duration_seconds: Some(10.0),
                    representations: vec![a],
                    event_streams: Vec::new(),
                },
            ],
            ..Manifest::default()
        };
        let xml = m.to_xml();
        assert!(xml.contains("<Period id=\"p0\""));
        assert!(xml.contains("<Period id=\"p1\""));
        assert!(xml.contains("start=\"PT10S\""));
        assert!(xml.contains("schemeIdUri=\"urn:scte:scte35:2014:xml+bin\""));
        assert!(xml.contains("BASE64SCTE"));
    }

    #[test]
    fn trick_play_and_ll_dash_fields() {
        let mut main = Representation::new(
            "0",
            video_stream(),
            "init_0.mp4",
            "seg_0_$Number$.m4s",
            90000,
            vec![540_000],
        );
        main.set_availability_time_offset(Some(3.5));
        let mut trick = Representation::new(
            "0_trick",
            video_stream(),
            "init_0_trick.mp4",
            "seg_0_trick_$Number$.m4s",
            90000,
            vec![540_000],
        );
        trick.max_playout_rate = Some(8.0);
        let xml = Manifest::static_vod(6.0, vec![main, trick], None).to_xml();
        assert!(xml.contains("availabilityTimeOffset=\"3.5\""));
        assert!(xml.contains("availabilityTimeComplete=\"false\""));
        assert!(xml.contains("maxPlayoutRate=\"8\""));
        assert!(xml.contains("http://dashif.org/guidelines/trickmode"));
        // Two AdaptationSets for video (normal + trick).
        assert_eq!(xml.matches("contentType=\"video\"").count(), 2);
    }

    #[test]
    fn on_demand_segment_list_shape() {
        let rep = Representation::on_demand(
            "0",
            video_stream(),
            90000,
            vec![540_000, 540_000],
            SegmentBaseInfo {
                base_url: "rep_0.mp4".into(),
                init_range: (0, 799),
                index_range: None,
                media_ranges: vec![(800, 1999), (2000, 3199)],
            },
        );
        let mut m = Manifest::static_vod(12.0, vec![rep], None);
        m.profile = DashProfile::OnDemand;
        let xml = m.to_xml();
        assert!(xml.contains("isoff-on-demand:2011"));
        assert!(xml.contains("<BaseURL>rep_0.mp4</BaseURL>"));
        assert!(xml.contains("<SegmentList"));
        assert!(xml.contains("range=\"0-799\""));
        assert!(xml.contains("mediaRange=\"800-1999\""));
        assert!(xml.contains("mediaRange=\"2000-3199\""));
        assert!(!xml.contains("SegmentTemplate"));
    }

    #[test]
    fn iso8601_formats() {
        assert_eq!(iso8601_duration(0.0), "PT0S");
        assert_eq!(iso8601_duration(90.5), "PT1M30.5S");
        assert_eq!(iso8601_duration(3661.0), "PT1H1M1S");
    }
}
