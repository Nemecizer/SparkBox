//! Honest streaming speed, shared by Chat, the server's request log and the
//! benchmarks. The clock starts at the first streamed piece of output
//! (reasoning or text), and a rate is only given when there were enough pieces
//! over enough time to measure one: a reply that arrives in one burst (a
//! buffering proxy, a cached answer) has no meaningful tokens per second.

use std::time::{Duration, Instant};

/// Fewer streamed pieces than this and the timing says more about chunking than speed.
pub const MIN_DELTAS: u64 = 8;
/// A shorter span than this is all clock noise.
pub const MIN_SPAN: Duration = Duration::from_millis(250);

/// Tokens per second between the first and last streamed piece: the first token
/// opens the span, so `tokens − 1` arrive inside it.
pub fn tokens_per_sec(tokens: u64, deltas: u64, span: Duration) -> Option<f64> {
    if deltas < MIN_DELTAS || span < MIN_SPAN || tokens < 2 {
        return None;
    }
    Some((tokens - 1) as f64 / span.as_secs_f64())
}

/// Timing of one streamed reply.
#[derive(Clone, Debug)]
pub struct Meter {
    pub started: Instant,
    pub first: Option<Instant>,
    pub last: Option<Instant>,
    pub deltas: u64,
}

impl Default for Meter {
    fn default() -> Self {
        Meter::new()
    }
}

impl Meter {
    pub fn new() -> Meter {
        Meter { started: Instant::now(), first: None, last: None, deltas: 0 }
    }

    /// One streamed piece of output arrived.
    pub fn delta(&mut self) {
        self.delta_at(Instant::now());
    }

    pub fn delta_at(&mut self, at: Instant) {
        self.first.get_or_insert(at);
        self.last = Some(at);
        self.deltas += 1;
    }

    /// Time to the first piece of output.
    pub fn ttft(&self) -> Option<Duration> {
        self.first.map(|f| f - self.started)
    }

    /// First to last piece.
    pub fn span(&self) -> Option<Duration> {
        Some(self.last? - self.first?)
    }

    pub fn rate(&self, tokens: u64) -> Option<f64> {
        tokens_per_sec(tokens, self.deltas, self.span()?)
    }
}

/// The demo models count a token as four characters, like a rough BPE tokenizer.
pub fn demo_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_never_reads_as_an_impossible_speed() {
        // 70 tokens that all arrived within 6 ms after a 2 s wait (a proxy flushing its buffer).
        let mut m = Meter::new();
        let t0 = m.started + Duration::from_secs(2);
        for i in 0..70 {
            m.delta_at(t0 + Duration::from_micros(i * 80));
        }
        assert_eq!(m.rate(70), None, "no rate from a burst");
        assert_eq!(m.ttft(), Some(Duration::from_secs(2)));

        // Spread out over 1.6 s it's a believable 43 tok/s.
        let mut m = Meter::new();
        for i in 0..70u64 {
            m.delta_at(m.started + Duration::from_millis(300 + i * 1600 / 69));
        }
        let r = m.rate(70).unwrap();
        assert!((r - 69.0 / 1.6).abs() < 0.5, "{r}");
        assert!(r < 1000.0);
    }

    #[test]
    fn too_few_pieces_or_too_short_a_span_give_no_rate() {
        assert_eq!(tokens_per_sec(500, 7, Duration::from_secs(3)), None, "7 chunks");
        assert_eq!(tokens_per_sec(500, 40, Duration::from_millis(200)), None, "200 ms");
        assert_eq!(tokens_per_sec(1, 40, Duration::from_secs(3)), None, "one token has no span");
        assert_eq!(tokens_per_sec(9, 8, Duration::from_secs(1)), Some(8.0));
    }

    #[test]
    fn demo_tokens_are_four_characters() {
        assert_eq!(demo_tokens(""), 0);
        assert_eq!(demo_tokens("abcd"), 1);
        assert_eq!(demo_tokens("abcde"), 2);
    }
}
