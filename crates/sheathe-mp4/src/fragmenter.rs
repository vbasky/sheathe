//! Segment boundary policy and accumulation.
//!
//! The fragmenter consumes samples in decode order and cuts a new segment when
//! the policy says to — typically at a keyframe once a target duration has
//! elapsed, exactly like Shaka's `--segment_duration`.

use sheathe_core::{Result, Sample, StreamInfo};

/// How to choose segment boundaries.
#[derive(Debug, Clone, Copy)]
pub struct SegmentPolicy {
    /// Target segment duration in seconds.
    pub target_seconds: f64,
    /// Only cut on keyframes (required for clean DASH/HLS switching).
    pub keyframes_only: bool,
}

impl Default for SegmentPolicy {
    fn default() -> Self {
        Self { target_seconds: 6.0, keyframes_only: true }
    }
}

/// One emitted media segment: the samples it contains and its timing.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Presentation start time, in the stream's timescale.
    pub start_ticks: u64,
    /// Total duration, in the stream's timescale.
    pub duration_ticks: u64,
    /// The samples belonging to this segment, in decode order.
    pub samples: Vec<Sample>,
}

/// Stateful accumulator that slices a sample stream into [`Segment`]s.
#[derive(Debug)]
pub struct Fragmenter {
    stream: StreamInfo,
    policy: SegmentPolicy,
    target_ticks: u64,
    current: Vec<Sample>,
    current_start: u64,
    done: Vec<Segment>,
}

impl Fragmenter {
    /// Create a fragmenter for `stream` using `policy`.
    pub fn new(stream: StreamInfo, policy: SegmentPolicy) -> Self {
        let scale = u64::from(stream.timescale.0);
        let target_ticks = (policy.target_seconds * scale as f64) as u64;
        Self {
            stream,
            policy,
            target_ticks,
            current: Vec::new(),
            current_start: 0,
            done: Vec::new(),
        }
    }

    /// The stream this fragmenter is segmenting.
    pub fn stream(&self) -> &StreamInfo {
        &self.stream
    }

    /// Feed the next sample in decode order.
    pub fn push(&mut self, sample: Sample) -> Result<()> {
        let elapsed = sample.dts.saturating_sub(self.current_start);
        let may_cut = !self.policy.keyframes_only || sample.is_segment_boundary();
        if !self.current.is_empty() && may_cut && elapsed >= self.target_ticks {
            self.cut(sample.dts);
        }
        if self.current.is_empty() {
            self.current_start = sample.dts;
        }
        self.current.push(sample);
        Ok(())
    }

    /// Flush the final partial segment and return all segments.
    pub fn finish(mut self) -> Vec<Segment> {
        if !self.current.is_empty() {
            let end = self.current.last().map(next_dts).unwrap_or(self.current_start);
            self.cut(end);
        }
        self.done
    }

    fn cut(&mut self, end_dts: u64) {
        let samples = std::mem::take(&mut self.current);
        self.done.push(Segment {
            start_ticks: self.current_start,
            duration_ticks: end_dts.saturating_sub(self.current_start),
            samples,
        });
    }
}

fn next_dts(s: &Sample) -> u64 {
    s.dts + u64::from(s.duration)
}
