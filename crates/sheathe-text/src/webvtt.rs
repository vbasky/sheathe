//! WebVTT parsing and TTML passthrough, both producing ISO/IEC 14496-30
//! (`wvtt` / `stpp`) sample entry and cue samples.

use sheathe_core::{Codec, Error, MediaKind, Result, Sample, SampleFlags, StreamInfo, Timescale};

/// A demuxed timed-text track (WebVTT).
pub struct TextTrack {
    /// Format-agnostic stream description.
    pub info: StreamInfo,
    /// Cue samples in presentation order (gapless; empty gaps are `vtte`).
    pub samples: Vec<Sample>,
    /// `wvtt` sample-entry box bytes for the CMAF init segment.
    pub sample_entry: Vec<u8>,
}

/// One parsed WebVTT cue.
struct Cue {
    start_ms: u64,
    end_ms: u64,
    settings: Option<String>,
    payload: String,
}

/// Parse a WebVTT document into a timed-text track.
pub fn webvtt(text: &str) -> Result<TextTrack> {
    if !text.trim_start().starts_with("WEBVTT") {
        return Err(Error::malformed("WebVTT: missing 'WEBVTT' signature"));
    }
    let cues = parse_cues(text);
    if cues.is_empty() {
        return Err(Error::malformed("WebVTT: no cues found"));
    }

    // Build a gapless sample timeline: `vtte` fills any gap before/between cues.
    let mut samples = Vec::new();
    let mut cursor_ms = 0u64;
    for cue in &cues {
        if cue.start_ms > cursor_ms {
            samples.push(text_sample(cursor_ms, cue.start_ms, vtte_box()));
        }
        let body = vttc_box(cue.settings.as_deref(), &cue.payload);
        samples.push(text_sample(cue.start_ms, cue.end_ms.max(cue.start_ms + 1), body));
        cursor_ms = cue.end_ms.max(cursor_ms);
    }

    let info = StreamInfo {
        kind: MediaKind::Text,
        codec: sheathe_core::Codec::WebVtt,
        timescale: Timescale::MPEG_TS,
        resolution: None,
        sample_rate: None,
        bitrate: None,
        codec_string: Some("wvtt".to_string()),
    };
    Ok(TextTrack { info, samples, sample_entry: wvtt_sample_entry("WEBVTT") })
}

/// Render decoded caption cues `(start_ms, end_ms, text)` into a WebVTT
/// document — shared by the CEA-608/708 caption decoders.
pub(crate) fn render_cues(cues: &[(u64, u64, String)]) -> String {
    let mut out = String::from("WEBVTT\n");
    for (start, end, text) in cues {
        out.push_str(&format!("\n{} --> {}\n{}\n", fmt_ts(*start), fmt_ts(*end), text));
    }
    out
}

/// Render cues that carry an optional WebVTT cue setting (e.g. `line:84.66%`),
/// appended to the timing line — used by the caption decoders for positioning.
pub(crate) fn render_positioned_cues(cues: &[(u64, u64, String, Option<String>)]) -> String {
    let mut out = String::from("WEBVTT\n");
    for (start, end, text, setting) in cues {
        let s = setting.as_deref().map(|x| format!(" {x}")).unwrap_or_default();
        out.push_str(&format!("\n{} --> {}{}\n{}\n", fmt_ts(*start), fmt_ts(*end), s, text));
    }
    out
}

/// Format milliseconds as WebVTT `HH:MM:SS.mmm`.
pub(crate) fn fmt_ts(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    format!("{h:02}:{m:02}:{s:02}.{:03}", ms % 1000)
}

/// A text sample spanning `[start_ms, end_ms)` at 90 kHz, carrying `body` bytes.
fn text_sample(start_ms: u64, end_ms: u64, body: Vec<u8>) -> Sample {
    let pts = start_ms * 90;
    let dur = ((end_ms - start_ms) * 90).max(1) as u32;
    Sample { dts: pts, pts, duration: dur, flags: SampleFlags::KEYFRAME, data: body }
}

fn parse_cues(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    // Split into blocks separated by blank lines; a cue block has a `-->` line.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for block in normalized.split("\n\n") {
        let mut lines = block.lines().peekable();
        // Skip an optional cue-identifier line (one without `-->`).
        let timing_line = loop {
            match lines.next() {
                Some(l) if l.contains("-->") => break Some(l),
                Some(_) => continue,
                None => break None,
            }
        };
        let Some(timing) = timing_line else { continue };
        let Some((start_ms, end_ms, settings)) = parse_timing(timing) else { continue };
        let payload = lines.collect::<Vec<_>>().join("\n");
        if payload.trim().is_empty() {
            continue;
        }
        cues.push(Cue { start_ms, end_ms, settings, payload });
    }
    cues.sort_by_key(|c| c.start_ms);
    cues
}

/// Parse a `HH:MM:SS.mmm --> HH:MM:SS.mmm settings…` line.
fn parse_timing(line: &str) -> Option<(u64, u64, Option<String>)> {
    let (lhs, rhs) = line.split_once("-->")?;
    let start = parse_ts(lhs.trim())?;
    let mut rest = rhs.trim().splitn(2, char::is_whitespace);
    let end = parse_ts(rest.next()?.trim())?;
    let settings = rest.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Some((start, end, settings))
}

/// Parse `[HH:]MM:SS.mmm` into milliseconds.
fn parse_ts(s: &str) -> Option<u64> {
    let (hms, ms) = s.split_once('.')?;
    let ms: u64 = ms.parse().ok()?;
    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec): (u64, u64, u64) = match parts.as_slice() {
        [h, m, s] => (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?),
        [m, s] => (0, m.parse().ok()?, s.parse().ok()?),
        _ => return None,
    };
    Some(((h * 3600 + m * 60 + sec) * 1000) + ms)
}

fn box_bytes(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// `wvtt` WebVTTSampleEntry with a `vttC` configuration box.
fn wvtt_sample_entry(config: &str) -> Vec<u8> {
    let vttc = box_bytes(b"vttC", config.as_bytes());
    let mut body = Vec::new();
    body.extend_from_slice(&[0; 6]); // reserved
    body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    body.extend_from_slice(&vttc);
    box_bytes(b"wvtt", &body)
}

/// `vttc` VTTCueBox: a `sttg` (settings, optional) + `payl` (payload).
fn vttc_box(settings: Option<&str>, payload: &str) -> Vec<u8> {
    let mut inner = Vec::new();
    if let Some(s) = settings {
        inner.extend_from_slice(&box_bytes(b"sttg", s.as_bytes()));
    }
    inner.extend_from_slice(&box_bytes(b"payl", payload.as_bytes()));
    box_bytes(b"vttc", &inner)
}

/// `vtte` VTTEmptyCueBox (fills a gap with no active cue).
fn vtte_box() -> Vec<u8> {
    box_bytes(b"vtte", &[])
}

/// Passthrough a TTML / IMSC document as an `stpp` timed-text track.
///
/// The entire document becomes a single cue spanning `end_ms` (parsed from the
/// TTML `<tt>` element's `ttp:tickRate` / temporal attributes, or a default).
/// Returns an error if the document is not valid UTF-8 or the XML root is not
/// `<tt>`.
pub fn ttml(text: &str) -> Result<TextTrack> {
    let text = text.trim();
    // Skip an optional XML declaration before looking for <tt>.
    let body = text
        .strip_prefix("<?xml")
        .and_then(|s| {
            let end = s.find("?>")?;
            Some(s[end + 2..].trim_start())
        })
        .unwrap_or(text);
    if !body.starts_with("<tt") {
        return Err(Error::malformed("TTML: missing '<tt' root element"));
    }

    // Simple duration extraction: look for `dur="HH:MM:SS.mmm"` or similar
    // on/in the `<tt>` element. Fall back to a default 10s when absent.
    let mut dur_ms = 10_000u64;
    if let Some(dur_str) = extract_attr(text, "dur=\"") {
        if let Some(d) = parse_smpte_dur(dur_str) {
            dur_ms = d.max(1);
        }
    }

    let sample = text_sample(0, dur_ms, box_bytes(b"payl", text.as_bytes()));
    let info = StreamInfo {
        kind: MediaKind::Text,
        codec: Codec::Stpp,
        timescale: Timescale::MPEG_TS,
        resolution: None,
        sample_rate: None,
        bitrate: None,
        codec_string: Some("stpp".to_string()),
    };
    Ok(TextTrack { info, samples: vec![sample], sample_entry: stpp_sample_entry() })
}

/// `stpp` TTMLSampleEntry with namespace/schema/aux fields (all empty).
fn stpp_sample_entry() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&[0; 6]); // reserved
    body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    // stpp-specific: namespace, schema_location, auxiliary_mime_types (null-terminated).
    body.push(0); // empty namespace
    body.push(0); // empty schema_location
    body.push(0); // empty auxiliary_mime_types
    box_bytes(b"stpp", &body)
}

/// Crude attribute-value extractor: find `key`, return content until `"` or `>`.
fn extract_attr<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let start = s.find(key)?;
    let val_start = start + key.len();
    let end = s[val_start..].find(['"', '>'])?;
    Some(&s[val_start..val_start + end])
}

/// Parse a SMPTE `HH:MM:SS.mmm` duration string to milliseconds.
fn parse_smpte_dur(s: &str) -> Option<u64> {
    // Support: HH:MM:SS.mmm, HH:MM:SS, MM:SS.mmm, or MM:SS
    let s = s.trim();
    let (hms, ms) =
        if let Some((h, m)) = s.split_once('.') { (h, m.parse().ok()?) } else { (s, 0) };
    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec): (u64, u64, u64) = match parts.as_slice() {
        [h, m, s] => (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?),
        [m, s] => (0, m.parse().ok()?, s.parse().ok()?),
        _ => return None,
    };
    Some(((h * 3600 + m * 60 + sec) * 1000) + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "WEBVTT\n\n\
        1\n00:00:01.000 --> 00:00:04.000\nHello world\n\n\
        00:00:05.000 --> 00:00:08.000 align:center\nSecond cue";

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_ts("00:00:01.000"), Some(1000));
        assert_eq!(parse_ts("01:02:03.500"), Some(3_723_500));
        assert_eq!(parse_ts("00:05.250"), Some(5_250));
    }

    #[test]
    fn builds_track_with_gaps_and_cues() {
        let t = webvtt(DOC).expect("parse");
        assert_eq!(t.info.kind, MediaKind::Text);
        assert_eq!(&t.sample_entry[4..8], b"wvtt");
        assert!(t.sample_entry.windows(4).any(|w| w == b"vttC"));
        // vtte (0–1s), cue1 (1–4s), vtte (4–5s), cue2 (5–8s) = 4 samples.
        assert_eq!(t.samples.len(), 4);
        assert_eq!(t.samples[0].pts, 0); // leading gap
        assert!(t.samples[0].data.windows(4).any(|w| w == b"vtte"));
        assert_eq!(t.samples[1].pts, 1000 * 90);
        assert_eq!(t.samples[1].duration, 3000 * 90);
        assert!(t.samples[1].data.windows(4).any(|w| w == b"payl"));
        // Second cue carries its settings in an `sttg` box.
        assert!(t.samples[3].data.windows(4).any(|w| w == b"sttg"));
    }

    #[test]
    fn rejects_non_webvtt() {
        assert!(webvtt("not a vtt file").is_err());
    }
}
