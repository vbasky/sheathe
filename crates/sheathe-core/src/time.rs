//! Media timing primitives.
//!
//! All timestamps in sheathe are integers expressed in a per-stream
//! [`Timescale`] (ticks per second), matching how ISO-BMFF and MPEG-TS carry
//! time. Avoiding floating point keeps segment boundaries exact.

/// Ticks-per-second for a stream's timeline (e.g. 90_000 for MPEG-TS video).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Timescale(pub u32);

impl Timescale {
    /// A common video timescale (90 kHz), as used by MPEG-2 transport streams.
    pub const MPEG_TS: Timescale = Timescale(90_000);

    /// Convert a duration in this timescale to whole milliseconds (truncating).
    pub fn to_millis(self, ticks: u64) -> u64 {
        ticks.saturating_mul(1_000) / u64::from(self.0.max(1))
    }
}

/// A value (timestamp or duration) paired with the [`Timescale`] it lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scaled {
    /// The raw tick count.
    pub ticks: u64,
    /// The timescale `ticks` are expressed in.
    pub scale: Timescale,
}

impl Scaled {
    /// Pair a tick count with its timescale.
    pub fn new(ticks: u64, scale: Timescale) -> Self {
        Self { ticks, scale }
    }

    /// Seconds as an `f64`, for display only — never for boundary math.
    pub fn seconds(self) -> f64 {
        self.ticks as f64 / f64::from(self.scale.0.max(1))
    }
}
