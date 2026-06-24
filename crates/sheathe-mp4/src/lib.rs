//! ISO base media file format (ISO/IEC 14496-12) writing for **sheathe**.
//!
//! This is the CMAF/fMP4 muxer: it turns a stream of [`Sample`]s into
//! initialization (`ftyp`+`moov`) and media (`moof`+`mdat`) segments, the
//! building blocks DASH and HLS reference. It corresponds to Shaka Packager's
//! `media/formats/mp4` plus the chunking layer.
//!
//! Read side: [`Mp4Demuxer`]. Write side: [`write_init_segment`] /
//! [`write_media_segment`]. Segmentation policy: [`Fragmenter`].

use sheathe_core::{Result, Sample, StreamInfo};

mod box_reader;
mod box_writer;
mod codecs;
mod demux;
mod fragmenter;
mod segment;

pub use box_reader::{BoxIter, Cursor, Mp4Box, top_level};
pub use box_writer::{BoxWriter, FourCc};
pub use demux::{Mp4Demuxer, Track};
pub use fragmenter::{Fragmenter, Segment, SegmentPolicy};
pub use segment::{Encryption, write_init_segment, write_media_segment};

/// Group `samples` into media segments according to `policy`.
pub fn fragment(
    stream: &StreamInfo,
    samples: Vec<Sample>,
    policy: SegmentPolicy,
) -> Result<Vec<Segment>> {
    let mut f = Fragmenter::new(stream.clone(), policy);
    for s in samples {
        f.push(s)?;
    }
    Ok(f.finish())
}
