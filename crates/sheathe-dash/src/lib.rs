//! MPEG-DASH (ISO/IEC 23009-1) manifest generation for **sheathe**.
//!
//! Mirrors Shaka Packager's `mpd` library: it takes the set of packaged
//! representations and emits a static (on-demand) `.mpd` using
//! `SegmentTemplate` + `SegmentTimeline`, which describes each segment's exact
//! duration — correct even when the last segment is short. Output is
//! differential-tested against Shaka Packager's MPD on a sample corpus.

use sheathe_core::{MediaKind, StreamInfo};
use std::fmt::Write as _;

/// One selectable rendition within an adaptation set.
#[derive(Debug, Clone)]
pub struct Representation {
    /// A unique id within the manifest (also used in segment URLs).
    pub id: String,
    /// The stream this representation carries.
    pub stream: StreamInfo,
    /// Initialization segment URL.
    pub init: String,
    /// Media segment URL template (may contain `$Number$`).
    pub media: String,
    /// Timescale the segment durations are expressed in.
    pub timescale: u32,
    /// Per-segment durations, in `timescale` ticks, in order.
    pub segment_durations: Vec<u64>,
}

/// A complete on-demand presentation.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// Total media duration in seconds.
    pub duration_seconds: f64,
    /// All representations, grouped by kind into adaptation sets at render time.
    pub representations: Vec<Representation>,
}

impl Manifest {
    /// Serialize to an MPD XML string (static / on-demand profile).
    pub fn to_xml(&self) -> String {
        let mut s = String::new();
        s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        // SegmentTemplate + SegmentTimeline is the "live" profile, even for
        // static (VOD) presentations.
        let _ = writeln!(
            s,
            concat!(
                "<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" ",
                "profiles=\"urn:mpeg:dash:profile:isoff-live:2011\" ",
                "type=\"static\" mediaPresentationDuration=\"{}\" ",
                "minBufferTime=\"PT2S\">"
            ),
            iso8601_duration(self.duration_seconds),
        );
        s.push_str("  <Period>\n");

        for (kind, content_type) in [
            (MediaKind::Video, "video"),
            (MediaKind::Audio, "audio"),
            (MediaKind::Text, "text"),
        ] {
            let reps: Vec<&Representation> = self
                .representations
                .iter()
                .filter(|r| r.stream.kind == kind)
                .collect();
            if reps.is_empty() {
                continue;
            }
            let _ = writeln!(
                s,
                "    <AdaptationSet contentType=\"{}\" segmentAlignment=\"true\">",
                content_type
            );
            for r in reps {
                render_representation(&mut s, r);
            }
            s.push_str("    </AdaptationSet>\n");
        }

        s.push_str("  </Period>\n</MPD>\n");
        s
    }
}

fn render_representation(s: &mut String, r: &Representation) {
    let codec = r.stream.rfc6381();
    let bandwidth = r.stream.bitrate.unwrap_or(0);
    let _ = write!(
        s,
        "      <Representation id=\"{}\" codecs=\"{}\"",
        r.id, codec
    );
    if let Some((w, h)) = r.stream.resolution {
        let _ = write!(s, " width=\"{}\" height=\"{}\"", w, h);
    }
    if let Some(rate) = r.stream.sample_rate {
        let _ = write!(s, " audioSamplingRate=\"{}\"", rate);
    }
    let _ = writeln!(s, " bandwidth=\"{}\">", bandwidth);

    let _ = writeln!(
        s,
        "        <SegmentTemplate timescale=\"{}\" initialization=\"{}\" media=\"{}\" startNumber=\"1\">",
        r.timescale, r.init, r.media
    );
    s.push_str("          <SegmentTimeline>\n");
    render_timeline(s, &r.segment_durations);
    s.push_str("          </SegmentTimeline>\n");
    s.push_str("        </SegmentTemplate>\n");
    s.push_str("      </Representation>\n");
}

/// Emit `<S>` entries, collapsing runs of equal durations with `r=`.
fn render_timeline(s: &mut String, durations: &[u64]) {
    let mut t = 0u64;
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
fn iso8601_duration(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) as f64 / 1000.0;
    let mut out = String::from("PT");
    if h > 0 {
        let _ = write!(out, "{}H", h);
    }
    if m > 0 {
        let _ = write!(out, "{}M", m);
    }
    let _ = write!(out, "{}S", s);
    out
}
