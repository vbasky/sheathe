//! MPEG-2 transport stream demuxer for the **sheathe** packager.
//!
//! Parses PAT/PMT/PES, reassembles elementary streams, and emits
//! [`sheathe_core::Sample`]s suitable for CMAF fragmentation. This is the read
//! side for `.ts` / `.m2ts` inputs — Shaka Packager's MPEG-TS parser.

pub mod elementary;
pub mod packet;

mod aac_entry;
mod ac3;
mod ac3_entry;
mod adts;
mod annexb;
mod avcc;
mod bitstream;
mod demux;
mod eac3;
mod eac3_entry;
mod flac;
mod flac_entry;
mod hvcc;
mod mp3;
mod mp3_entry;
mod pes;
mod psi;

pub use demux::{TsDemuxer, TsTrack};
