//! Shaka-style stream descriptors (`in=file,stream=audio,language=eng,…`).
//!
//! A bare filesystem path is accepted as “all tracks in this file”.

use anyhow::{Context, Result, bail};
use sheathe_core::MediaKind;
use std::path::{Path, PathBuf};

/// Which tracks of an input to package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StreamSelector {
    /// Every audio, video, and text track (sheathe default).
    #[default]
    All,
    /// First track of this kind.
    Kind(MediaKind),
    /// Zero-based stream index in demuxer order.
    Index(usize),
}

/// Per-stream overrides mirroring Shaka Packager stream_descriptor fields.
#[derive(Debug, Clone, Default)]
pub struct StreamDescriptor {
    /// Input path (`input=` / `in=`).
    pub input: PathBuf,
    /// Track selector (`stream=`).
    pub selector: StreamSelector,
    /// Optional output / init name (`output=` / `init_segment=`).
    pub output: Option<String>,
    /// Optional segment name template (`segment_template=`). `$Number$` is replaced.
    pub segment_template: Option<String>,
    /// Manifest bandwidth override, bits/sec (`bandwidth=` / `bw=`).
    pub bandwidth: Option<u32>,
    /// Language tag override (`language=` / `lang=`).
    pub language: Option<String>,
    /// Skip CENC for this stream (`skip_encryption=1`).
    pub skip_encryption: bool,
    /// DRM key label (`drm_label=`): AUDIO, SD, HD, UHD1, UHD2, or a `--keys` label.
    pub drm_label: Option<String>,
    /// Trick-play factor: keep every Nth keyframe (`trick_play_factor=` / `tpf=`).
    pub trick_play_factor: Option<u32>,
    /// HLS `#EXT-X-MEDIA` NAME (`hls_name=`).
    pub hls_name: Option<String>,
    /// HLS `#EXT-X-MEDIA` GROUP-ID (`hls_group_id=`).
    pub hls_group_id: Option<String>,
    /// HLS media playlist filename (`playlist_name=`).
    pub playlist_name: Option<String>,
    /// HLS I-frame playlist filename (`iframe_playlist_name=`).
    pub iframe_playlist_name: Option<String>,
    /// HLS CHARACTERISTICS (`hls_characteristics=` / `charcs=`), colon/semicolon split.
    pub hls_characteristics: Vec<String>,
    /// DASH Accessibility `schemeIdUri=value` (`dash_accessibilities=`).
    pub dash_accessibilities: Vec<(String, String)>,
    /// DASH Role values (`dash_roles=`).
    pub dash_roles: Vec<String>,
    /// Forced narrative subtitle (`forced_subtitle=1`).
    pub forced_subtitle: bool,
}

impl StreamDescriptor {
    /// Package every track in `path`.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self { input: path.into(), ..Self::default() }
    }

    /// True when this descriptor selects `kind` at demuxer index `index`.
    pub fn matches(&self, index: usize, kind: MediaKind) -> bool {
        match self.selector {
            StreamSelector::All => true,
            StreamSelector::Kind(k) => k == kind,
            StreamSelector::Index(i) => i == index,
        }
    }
}

/// Parse a CLI argument: a filesystem path, or a Shaka `key=value,key=value` descriptor.
pub fn parse_input_arg(arg: &str) -> Result<StreamDescriptor> {
    if Path::new(arg).exists() || !arg.contains('=') {
        return Ok(StreamDescriptor::from_path(arg));
    }
    parse_descriptor(arg)
}

/// Parse a Shaka stream_descriptor string.
pub fn parse_descriptor(spec: &str) -> Result<StreamDescriptor> {
    let mut d = StreamDescriptor::default();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .with_context(|| format!("stream descriptor field '{part}' is not key=value"))?;
        let key = key.trim();
        let value = value.trim();
        match key {
            "input" | "in" => d.input = PathBuf::from(value),
            "stream" | "stream_selector" => d.selector = parse_selector(value)?,
            "output" | "out" | "init_segment" => d.output = Some(value.to_string()),
            "segment_template" | "segment" => d.segment_template = Some(value.to_string()),
            "bandwidth" | "bw" => {
                d.bandwidth = Some(value.parse().context("bandwidth must be an integer")?);
            }
            "language" | "lang" => d.language = Some(value.to_string()),
            "skip_encryption" => d.skip_encryption = value != "0" && value != "false",
            "drm_label" => d.drm_label = Some(value.to_string()),
            "trick_play_factor" | "tpf" => {
                d.trick_play_factor = Some(value.parse().context("trick_play_factor")?);
            }
            "hls_name" => d.hls_name = Some(value.to_string()),
            "hls_group_id" => d.hls_group_id = Some(value.to_string()),
            "playlist_name" => d.playlist_name = Some(value.to_string()),
            "iframe_playlist_name" => d.iframe_playlist_name = Some(value.to_string()),
            "hls_characteristics" | "charcs" => {
                d.hls_characteristics =
                    value.split([':', ';']).filter(|s| !s.is_empty()).map(str::to_string).collect();
            }
            "dash_accessibilities" | "accessibilities" => {
                for item in value.split(';').filter(|s| !s.is_empty()) {
                    let (scheme, v) = item
                        .split_once('=')
                        .context("dash_accessibilities entries must be scheme_id_uri=value")?;
                    d.dash_accessibilities.push((scheme.to_string(), v.to_string()));
                }
            }
            "dash_roles" | "roles" => {
                d.dash_roles =
                    value.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect();
            }
            "forced_subtitle" => d.forced_subtitle = value != "0" && value != "false",
            "output_format" | "input_format" | "format" => { /* accepted, ignored */ }
            other => bail!("unknown stream descriptor field '{other}'"),
        }
    }
    anyhow::ensure!(!d.input.as_os_str().is_empty(), "stream descriptor missing input=/in=");
    Ok(d)
}

fn parse_selector(value: &str) -> Result<StreamSelector> {
    match value.to_ascii_lowercase().as_str() {
        "audio" => Ok(StreamSelector::Kind(MediaKind::Audio)),
        "video" => Ok(StreamSelector::Kind(MediaKind::Video)),
        "text" => Ok(StreamSelector::Kind(MediaKind::Text)),
        n => {
            let i: usize = n.parse().with_context(|| {
                format!("stream selector '{value}' (expected audio, video, text, or an index)")
            })?;
            Ok(StreamSelector::Index(i))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_path() {
        let d = parse_input_arg("movie.mp4").unwrap();
        assert_eq!(d.input, PathBuf::from("movie.mp4"));
        assert_eq!(d.selector, StreamSelector::All);
    }

    #[test]
    fn shaka_descriptor() {
        let d = parse_descriptor(
            "in=movie.mp4,stream=audio,language=eng,hls_name=English,drm_label=AUDIO,skip_encryption=1",
        )
        .unwrap();
        assert_eq!(d.input, PathBuf::from("movie.mp4"));
        assert_eq!(d.selector, StreamSelector::Kind(MediaKind::Audio));
        assert_eq!(d.language.as_deref(), Some("eng"));
        assert_eq!(d.hls_name.as_deref(), Some("English"));
        assert_eq!(d.drm_label.as_deref(), Some("AUDIO"));
        assert!(d.skip_encryption);
    }

    #[test]
    fn roles_and_accessibilities() {
        let d = parse_descriptor(
            "in=a.mp4,stream=text,dash_roles=subtitle;forced-subtitle,dash_accessibilities=urn:foo=bar,forced_subtitle=1",
        )
        .unwrap();
        assert_eq!(d.dash_roles, vec!["subtitle", "forced-subtitle"]);
        assert_eq!(d.dash_accessibilities, vec![("urn:foo".into(), "bar".into())]);
        assert!(d.forced_subtitle);
    }
}
