//! Raw elementary stream demuxer.

use crate::detect::StreamKind;
use sheathe_core::{Error, Result};
use sheathe_ts::elementary::{self, ElementaryTrack};

/// A demuxed raw elementary stream (single track).
pub struct EsDemuxer {
    track: ElementaryTrack,
}

impl EsDemuxer {
    /// Parse `data` as the given elementary stream kind.
    pub fn parse(data: &[u8], kind: StreamKind) -> Result<Self> {
        let track = match kind {
            StreamKind::H264AnnexB => elementary::h264_from_annex_b(data, &[])?,
            StreamKind::HevcAnnexB => elementary::hevc_from_annex_b(data, &[])?,
            StreamKind::AacAdts => elementary::aac_adts(data)?,
            StreamKind::Ac3 => elementary::ac3(data)?,
            StreamKind::Eac3 => elementary::eac3(data)?,
            StreamKind::Mp3 => elementary::mp3(data)?,
            StreamKind::Flac => elementary::flac(data)?,
        };
        Ok(Self { track })
    }

    /// Detect stream kind from `path` + `data`, then parse.
    pub fn parse_auto(path: &str, data: &[u8]) -> Result<Self> {
        let kind = crate::detect::detect(path, data).ok_or_else(|| {
            Error::malformed(format!("could not detect elementary stream type for {path}"))
        })?;
        Self::parse(data, kind)
    }

    /// Parsed track.
    pub fn track(&self) -> &ElementaryTrack {
        &self.track
    }
}
