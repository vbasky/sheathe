//! Fuzz MPEG-TS packet parsing.
#![no_main]
use libfuzzer_sys::fuzz_target;
use sheathe_ts::{TsDemuxer, packet::packets};

fuzz_target!(|data: &[u8]| {
    for p in packets(data) {
        let _ = p;
    }
    let _ = TsDemuxer::parse(data);
});
