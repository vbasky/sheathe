//! Raw elementary stream demuxer for the **sheathe** packager.
//!
//! Parses Annex B H.264/H.265 and ADTS-AAC byte streams into
//! [`sheathe_core::Sample`]s and MP4 sample entries — the read side for `.h264`,
//! `.hevc`, `.aac`, and related extensions.

mod demux;
mod detect;

pub use demux::EsDemuxer;
pub use detect::{StreamKind, detect, is_mp4};
pub use sheathe_ts::elementary::ElementaryTrack;