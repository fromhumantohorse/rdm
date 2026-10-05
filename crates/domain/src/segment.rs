use serde::{Deserialize, Serialize};

/// Inclusive byte range `[start, end]`, with `pos` = next byte to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub pos: u64,
    pub end: u64,
}

impl Segment {
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, pos: start, end }
    }

    pub const fn is_done(&self) -> bool {
        self.pos > self.end
    }
}

/// Splits `size` bytes into at most `n` segments, none smaller than `min_len`.
pub fn plan_segments(size: u64, n: u8, min_len: u64) -> Vec<Segment> {
    if size == 0 {
        return Vec::new();
    }
    let n = u64::from(n.max(1)).min(size.div_ceil(min_len.max(1)));
    let chunk = size.div_ceil(n);
    (0..n)
        .map(|i| i * chunk)
        .take_while(|&s| s < size)
        .map(|s| Segment::new(s, s.saturating_add(chunk).min(size) - 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_whole_range_without_gaps() {
        let segs = plan_segments(10_000_001, 32, 1);
        assert_eq!(segs.first().unwrap().start, 0);
        assert_eq!(segs.last().unwrap().end, 10_000_000);
        assert!(segs.windows(2).all(|w| w[0].end + 1 == w[1].start));
    }

    #[test]
    fn no_piece_is_smaller_than_the_minimum() {
        assert_eq!(plan_segments(1000, 32, 500).len(), 2);
    }

    #[test]
    fn a_segment_is_done_once_its_last_byte_is_fetched() {
        let mut segment = Segment::new(10, 19);
        assert!(!segment.is_done(), "nothing fetched");

        segment.pos = 19;
        assert!(!segment.is_done(), "the last byte still to fetch");

        segment.pos = 20;
        assert!(segment.is_done());
    }

    /// More connections than the size divides into evenly: no piece is planned past the end.
    #[test]
    fn never_plans_an_empty_piece() {
        let segments = plan_segments(10, 6, 1);

        assert_eq!(segments.len(), 5);
        assert!(segments.iter().all(|s| s.start <= s.end && s.end < 10));
    }

    /// A size the server made up (the largest a header can say) is split like any other.
    #[test]
    fn plans_the_largest_size_without_overflowing() {
        let segs = plan_segments(u64::MAX, 64, 1);

        assert_eq!(segs.len(), 64);
        assert_eq!(segs.last().unwrap().end, u64::MAX - 1);
        assert!(segs.windows(2).all(|w| w[0].end + 1 == w[1].start));
    }
}
