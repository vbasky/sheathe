//! WebVTT parsing and ISO/IEC 14496-30 (`wvtt`) sample synthesis.

use sheathe_core::{Error, MediaKind, Result, Sample, SampleFlags, StreamInfo, Timescale};

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
