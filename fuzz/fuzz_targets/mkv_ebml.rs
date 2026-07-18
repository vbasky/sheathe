//! Fuzz WebM/Matroska EBML demux.
#![no_main]
use libfuzzer_sys::fuzz_target;
use sheathe_mkv::MkvDemuxer;

fuzz_target!(|data: &[u8]| {
    let _ = MkvDemuxer::parse(data);
});
