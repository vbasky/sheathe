//! WebM/Matroska demuxer for the **sheathe** packager.
//!
//! Parses the EBML container, reads the track list, and extracts
//! [`sheathe_core::Sample`]s from clusters — the read side for `.webm` / `.mkv`
//! inputs. Supports VP8/VP9/AV1 video and Opus audio, synthesising the matching
//! `vp08`/`vp09`/`av01`/`Opus` sample entries (with `vpcC`/`av1C`/`dOps`).

mod demux;
mod ebml;
mod entries;

pub use demux::{MkvDemuxer, MkvTrack, is_webm};
