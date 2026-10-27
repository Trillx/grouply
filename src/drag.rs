//! Delayed drop targeting: a dragged window only groups with a target after hovering over that
//! target's drop zone for a while, which keeps ordinary window moves from grouping by accident.

#[derive(Debug)]
pub struct DropTracker<W> {
    hold_ms: u64,
    candidate: Option<(W, u64)>,
}

impl<W: Copy + Eq> DropTracker<W> {
    pub fn new(hold_ms: u64) -> Self {
        Self {
            hold_ms,
            candidate: None,
        }
    }

    /// Records what is under the cursor at `now_ms`. Changing targets restarts the hold.
    pub fn update(&mut self, target: Option<W>, now_ms: u64) {
        self.candidate = match (self.candidate, target) {
            (Some((current, since)), Some(t)) if current == t => Some((current, since)),
            (_, Some(t)) => Some((t, now_ms)),
            (_, None) => None,
        };
    }

    /// The current target, once it has been held for long enough.
    pub fn armed(&self, now_ms: u64) -> Option<W> {
        self.candidate
            .filter(|&(_, since)| now_ms.saturating_sub(since) >= self.hold_ms)
            .map(|(w, _)| w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arms_after_hold_time() {
        let mut t = DropTracker::new(300);
        t.update(Some(7), 1000);
        assert_eq!(t.armed(1299), None);
        t.update(Some(7), 1200);
        assert_eq!(t.armed(1300), Some(7));
    }

    #[test]
    fn switching_targets_restarts_the_hold() {
        let mut t = DropTracker::new(300);
        t.update(Some(1), 0);
        t.update(Some(2), 250);
        assert_eq!(t.armed(400), None);
        assert_eq!(t.armed(550), Some(2));
    }

    #[test]
    fn leaving_the_zone_disarms() {
        let mut t = DropTracker::new(0);
        t.update(Some(1), 0);
        t.update(None, 10);
        assert_eq!(t.armed(20), None);
        t.update(Some(1), 30);
        assert_eq!(t.armed(30), Some(1), "re-entering starts a fresh hold");
    }

    #[test]
    fn zero_hold_arms_immediately() {
        let mut t = DropTracker::new(0);
        t.update(Some(3), 5);
        assert_eq!(t.armed(5), Some(3));
    }
}
