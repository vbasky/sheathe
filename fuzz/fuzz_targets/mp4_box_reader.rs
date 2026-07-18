//! Fuzz the ISO-BMFF top-level box walker.
#![no_main]
use libfuzzer_sys::fuzz_target;
use sheathe_mp4::{Mp4Demuxer, top_level};

fuzz_target!(|data: &[u8]| {
    // Structural walk — must never panic.
    let _ = top_level(data).count();
    // Full demux path (may return Err).
    let _ = Mp4Demuxer::parse(data);
});
