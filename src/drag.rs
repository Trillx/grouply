//! Delayed drop targeting: a dragged window only groups with a target after hovering over that
//! target's drop zone for a while, which keeps ordinary window moves from grouping by accident.
//! Only moves group, so the dragged window's size is watched to tell them from resizes.

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

/// Tells a moved window from a resized one by its size during the drag.
///
/// Dragging a maximized or snapped window first restores its earlier size, so one change is
/// expected. A resize keeps changing it, and a change at release means Windows snapped the window
/// where it was dropped.
#[derive(Debug)]
pub struct SizeWatch {
    last: Option<(i32, i32)>,
    changes: u32,
}

impl SizeWatch {
    pub fn new(size: Option<(i32, i32)>) -> Self {
        Self {
            last: size,
            changes: 0,
        }
    }

    /// Records the size seen while the drag is under way. Unknown sizes are skipped.
    pub fn observe(&mut self, size: Option<(i32, i32)>) {
        let Some(size) = size else { return };
        if self.last.is_some_and(|last| last != size) {
            self.changes += 1;
        }
        self.last = Some(size);
    }

    /// Whether a drag that ends with the window at `end` size was something other than a move.
    pub fn resized(&self, end: Option<(i32, i32)>) -> bool {
        let snapped = matches!((self.last, end), (Some(a), Some(b)) if a != b);
        self.changes > 1 || snapped
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

    #[test]
    fn a_plain_move_is_not_a_resize() {
        let mut s = SizeWatch::new(Some((800, 600)));
        s.observe(Some((800, 600)));
        s.observe(Some((800, 600)));
        assert!(!s.resized(Some((800, 600))));
    }

    #[test]
    fn restoring_a_maximized_or_snapped_window_is_still_a_move() {
        let mut s = SizeWatch::new(Some((1920, 1040)));
        s.observe(Some((800, 600)));
        s.observe(Some((800, 600)));
        assert!(!s.resized(Some((800, 600))));
    }

    #[test]
    fn dragging_a_border_is_a_resize() {
        let mut s = SizeWatch::new(Some((800, 600)));
        s.observe(Some((820, 600)));
        s.observe(Some((850, 600)));
        assert!(s.resized(Some((850, 600))));
    }

    #[test]
    fn a_snap_on_release_is_a_resize() {
        let mut s = SizeWatch::new(Some((800, 600)));
        s.observe(Some((800, 600)));
        assert!(s.resized(Some((960, 1040))));

        let mut restored = SizeWatch::new(Some((1920, 1040)));
        restored.observe(Some((800, 600)));
        assert!(restored.resized(Some((960, 1040))));
    }

    #[test]
    fn unknown_sizes_are_ignored() {
        let mut s = SizeWatch::new(None);
        s.observe(Some((800, 600)));
        s.observe(None);
        assert!(!s.resized(Some((800, 600))));
        assert!(!s.resized(None));
    }
}
