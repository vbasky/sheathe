//! A single coded media sample (access unit / frame) on a stream's timeline.

/// Per-sample flags relevant to fragmentation and segment boundaries.
///
/// A hand-rolled `u8` flag set, so `sheathe-core` needs no dependency beyond
/// `thiserror`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SampleFlags(u8);

impl SampleFlags {
    /// This sample is a sync sample (IDR / keyframe) — a valid segment start.
    pub const KEYFRAME: SampleFlags = SampleFlags(0b0000_0001);
    /// This sample is not depended on by others (droppable).
    pub const DISPOSABLE: SampleFlags = SampleFlags(0b0000_0010);

    /// The empty flag set.
    pub const fn empty() -> Self {
        SampleFlags(0)
    }

    /// Whether every bit in `other` is set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Set the bits in `other`.
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl core::ops::BitOr for SampleFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        SampleFlags(self.0 | rhs.0)
    }
}

/// One coded sample: its bytes plus the timing the muxer needs.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Decode timestamp, in the stream's timescale.
    pub dts: u64,
    /// Presentation timestamp, in the stream's timescale.
    pub pts: u64,
    /// Sample duration, in the stream's timescale.
    pub duration: u32,
    /// Flags describing the sample (keyframe, etc.).
    pub flags: SampleFlags,
    /// The coded bytes (NAL units, raw frame, …).
    pub data: Vec<u8>,
}

impl Sample {
    /// Whether this sample may begin a new segment.
    pub fn is_segment_boundary(&self) -> bool {
        self.flags.contains(SampleFlags::KEYFRAME)
    }
}
