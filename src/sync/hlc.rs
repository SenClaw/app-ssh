//! Hybrid logical clock: a stamp that orders edits across devices without
//! needing their wall clocks to agree.
//!
//! A stamp is `(milliseconds, counter, device)`. A device never issues a stamp
//! lower than one it has issued or received, so an edit made after seeing a
//! peer's edit always sorts after it, even when this device's clock is behind.
//! The device id breaks the remaining ties, so two stamps are never equal and
//! every replica picks the same winner.
//!
//! The text form is fixed-width, so comparing the strings compares the stamps.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hlc {
    pub ms: u64,
    pub counter: u32,
    pub device: String,
}

impl Hlc {
    pub fn parse(text: &str) -> Option<Hlc> {
        let mut parts = text.splitn(3, '-');
        let ms = parts.next()?.parse().ok()?;
        let counter = parts.next()?.parse().ok()?;
        let device = parts.next()?.to_string();
        if device.is_empty() || !device.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        Some(Hlc { ms, counter, device })
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:013}-{:05}-{}", self.ms, self.counter, self.device)
    }
}

/// The part of the clock that must survive a restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Clock {
    pub ms: u64,
    pub counter: u32,
}

impl Clock {
    /// Stamp a local edit.
    pub fn tick(&mut self, now_ms: u64, device: &str) -> Hlc {
        if now_ms > self.ms {
            self.ms = now_ms;
            self.counter = 0;
        } else {
            self.counter += 1;
        }
        Hlc { ms: self.ms, counter: self.counter, device: device.to_string() }
    }

    /// Move past a stamp that came from a peer, so the next local edit sorts
    /// after everything this device has seen.
    pub fn receive(&mut self, remote: &Hlc, now_ms: u64) {
        let ms = self.ms.max(remote.ms).max(now_ms);
        let counter = if ms == self.ms && ms == remote.ms {
            self.counter.max(remote.counter) + 1
        } else if ms == self.ms {
            self.counter + 1
        } else if ms == remote.ms {
            remote.counter + 1
        } else {
            0
        };
        self.ms = ms;
        self.counter = counter;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips_and_orders_like_the_stamp() {
        let a = Hlc { ms: 999, counter: 3, device: "aaaa".into() };
        let b = Hlc { ms: 1000, counter: 0, device: "aaaa".into() };
        assert_eq!(Hlc::parse(&a.to_string()), Some(a.clone()));
        assert!(a < b);
        assert!(a.to_string() < b.to_string());
        let c = Hlc { ms: 1000, counter: 0, device: "bbbb".into() };
        assert!(b < c && b.to_string() < c.to_string());
    }

    #[test]
    fn rejects_malformed_stamps() {
        for bad in ["", "1-2", "x-1-dev", "0000000000001-00001-", "0000000000001-00001-a/b"] {
            assert_eq!(Hlc::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn ticks_never_go_backwards_even_when_the_wall_clock_does() {
        let mut c = Clock::default();
        let first = c.tick(5000, "d");
        let same_ms = c.tick(5000, "d");
        let behind = c.tick(100, "d");
        assert!(first < same_ms && same_ms < behind);
    }

    #[test]
    fn an_edit_after_a_received_stamp_sorts_after_it() {
        let mut c = Clock::default();
        let far_ahead = Hlc { ms: 90_000, counter: 7, device: "peer".into() };
        c.receive(&far_ahead, 1_000);
        let mine = c.tick(1_000, "me");
        assert!(mine > far_ahead);
    }
}
