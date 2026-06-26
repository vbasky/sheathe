//! MPEG-2 transport stream demuxer for the **sheathe** packager.
//!
//! Parses PAT/PMT/PES, reassembles elementary streams, and emits
//! [`sheathe_core::Sample`]s suitable for CMAF fragmentation. This is the read
//! side for `.ts` / `.m2ts` inputs — Shaka Packager's MPEG-TS parser.

pub mod elementary;
pub mod packet;

mod aac_entry;
mod adts;
mod annexb;
mod avcc;
mod bitstream;
mod demux;
mod hvcc;
mod pes;
mod psi;

pub use demux::{TsDemuxer, TsTrack};