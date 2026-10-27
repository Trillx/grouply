//! Platform-independent geometry: where a group's tab bar goes and how tabs are laid out in it.
//!
//! All values are physical pixels. Metrics are authored at 96 DPI and scaled per monitor.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub const fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub const fn offset(&self, dx: i32, dy: i32) -> Self {
        Self::new(
            self.left + dx,
            self.top + dy,
            self.right + dx,
            self.bottom + dy,
        )
    }
}

/// Tab bar dimensions. Construct with [`BarMetrics::default`] (96 DPI) and call [`BarMetrics::scaled`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarMetrics {
    /// Total bar height, including the gap above the tabs.
    pub height: i32,
    /// Empty space above the tabs.
    pub top_gap: i32,
    /// Horizontal offset of the first tab from the window frame's left edge.
    pub inset: i32,
    pub tab_min: i32,
    pub tab_max: i32,
    /// Space between adjacent tabs.
    pub spacing: i32,
    /// Inner horizontal padding of a tab.
    pub padding: i32,
    pub icon: i32,
    pub close: i32,
    /// Width kept free on the right when the bar overlays a window's own caption buttons.
    pub caption_reserve: i32,
    pub corner: i32,
}

impl Default for BarMetrics {
    fn default() -> Self {
        Self {
            height: 32,
            top_gap: 4,
            inset: 8,
            tab_min: 72,
            tab_max: 220,
            spacing: 2,
            padding: 8,
            icon: 16,
            close: 16,
            caption_reserve: 150,
            corner: 8,
        }
    }
}

impl BarMetrics {
    pub fn scaled(&self, dpi: u32) -> Self {
        let s = |v: i32| (v as i64 * dpi.max(1) as i64 / 96) as i32;
        Self {
            height: s(self.height),
            top_gap: s(self.top_gap),
            inset: s(self.inset),
            tab_min: s(self.tab_min),
            tab_max: s(self.tab_max),
            spacing: s(self.spacing),
            padding: s(self.padding),
            icon: s(self.icon),
            close: s(self.close),
            caption_reserve: s(self.caption_reserve),
            corner: s(self.corner),
        }
    }
}

/// Screen rectangle for a group's tab bar.
///
/// The bar normally sits directly above the active window's visible frame. When there is no room
/// above it inside the monitor work area (maximized or top-snapped windows), the bar overlays the
/// top of the frame instead and keeps clear of the caption buttons.
pub fn bar_rect(frame: Rect, work_area: Rect, m: &BarMetrics) -> Rect {
    let fits_above = frame.top - m.height >= work_area.top;
    let (top, reserve) = if fits_above {
        (frame.top - m.height, 0)
    } else {
        (frame.top.max(work_area.top), m.caption_reserve)
    };
    let left = frame.left + m.inset;
    let right = (frame.right - m.inset - reserve).max(left);
    Rect::new(left, top, right, top + m.height)
}

/// One tab's parts, in bar-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabGeom {
    pub tab: Rect,
    pub icon: Rect,
    pub text: Rect,
    pub close: Rect,
}

/// Lays out `count` equal-width tabs, left-aligned, in a bar of `bar_width` pixels.
///
/// Tabs shrink to share the width but never below `tab_min`; tabs past the right edge are clipped.
pub fn layout_tabs(bar_width: i32, count: usize, m: &BarMetrics) -> Vec<TabGeom> {
    if count == 0 {
        return Vec::new();
    }
    let n = count as i32;
    let available = (bar_width - m.spacing * (n - 1)).max(0);
    let width = (available / n).clamp(m.tab_min, m.tab_max);
    let top = m.top_gap;
    let bottom = m.height;
    let mid = (top + bottom) / 2;

    (0..n)
        .map(|i| {
            let left = i * (width + m.spacing);
            let tab = Rect::new(left, top, left + width, bottom);
            let icon_left = left + m.padding;
            let icon = Rect::new(
                icon_left,
                mid - m.icon / 2,
                icon_left + m.icon,
                mid - m.icon / 2 + m.icon,
            );
            let close_right = tab.right - m.padding;
            let close = Rect::new(
                close_right - m.close,
                mid - m.close / 2,
                close_right,
                mid - m.close / 2 + m.close,
            );
            let text = Rect::new(
                icon.right + m.padding / 2,
                top,
                close.left - m.padding / 2,
                bottom,
            );
            TabGeom {
                tab,
                icon,
                text,
                close,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Close(usize),
    None,
}

pub fn hit_test(tabs: &[TabGeom], x: i32, y: i32) -> Hit {
    for (i, g) in tabs.iter().enumerate() {
        if g.close.contains(x, y) {
            return Hit::Close(i);
        }
        if g.tab.contains(x, y) {
            return Hit::Tab(i);
        }
    }
    Hit::None
}

/// Index a tab dragged from `from` should move to when released at bar-local `x`.
pub fn reorder_target(tabs: &[TabGeom], from: usize, x: i32) -> usize {
    let past = tabs
        .iter()
        .enumerate()
        .filter(|&(i, g)| i != from && x >= (g.tab.left + g.tab.right) / 2)
        .count();
    past.min(tabs.len().saturating_sub(1))
}

/// Whether screen point (x, y) is a place where dropping a dragged window groups it with `frame`'s
/// window: the caption band along the top of the frame, or that window's tab bar.
pub fn in_drop_zone(frame: Rect, bar: Option<Rect>, caption_height: i32, x: i32, y: i32) -> bool {
    let caption = Rect::new(
        frame.left,
        frame.top,
        frame.right,
        frame.top + caption_height,
    );
    caption.contains(x, y) || bar.is_some_and(|b| b.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);

    #[test]
    fn bar_sits_above_a_normal_window() {
        let m = BarMetrics::default();
        let bar = bar_rect(Rect::new(100, 200, 900, 800), WORK, &m);
        assert_eq!(bar, Rect::new(108, 168, 892, 200));
    }

    #[test]
    fn bar_overlays_a_maximized_window_and_avoids_caption_buttons() {
        let m = BarMetrics::default();
        let bar = bar_rect(Rect::new(0, 0, 1920, 1040), WORK, &m);
        assert_eq!(bar, Rect::new(8, 0, 1920 - 8 - 150, 32));
    }

    #[test]
    fn bar_never_has_negative_width() {
        let m = BarMetrics::default();
        let bar = bar_rect(Rect::new(10, 5, 60, 300), WORK, &m);
        assert!(bar.width() >= 0);
    }

    #[test]
    fn metrics_scale_with_dpi() {
        let m = BarMetrics::default().scaled(144);
        assert_eq!(m.height, 48);
        assert_eq!(m.tab_max, 330);
    }

    #[test]
    fn tabs_share_space_and_respect_limits() {
        let m = BarMetrics::default();
        let wide = layout_tabs(2000, 2, &m);
        assert_eq!(wide[0].tab.width(), m.tab_max);
        assert_eq!(wide[1].tab.left, m.tab_max + m.spacing);

        let narrow = layout_tabs(300, 3, &m);
        assert_eq!(narrow[0].tab.width(), (300 - 2 * m.spacing) / 3);

        let crowded = layout_tabs(100, 4, &m);
        assert!(crowded.iter().all(|g| g.tab.width() == m.tab_min));
    }

    #[test]
    fn tab_parts_stay_inside_the_tab() {
        let m = BarMetrics::default();
        for g in layout_tabs(800, 3, &m) {
            for part in [g.icon, g.text, g.close] {
                assert!(
                    part.left >= g.tab.left && part.right <= g.tab.right,
                    "{part:?} outside {:?}",
                    g.tab
                );
                assert!(part.top >= g.tab.top && part.bottom <= g.tab.bottom);
            }
            assert!(g.text.left < g.text.right);
        }
    }

    #[test]
    fn hit_test_prefers_close_button() {
        let m = BarMetrics::default();
        let tabs = layout_tabs(800, 2, &m);
        let c = tabs[1].close;
        assert_eq!(hit_test(&tabs, c.left + 1, c.top + 1), Hit::Close(1));
        assert_eq!(
            hit_test(&tabs, tabs[0].tab.left + 1, tabs[0].tab.top + 1),
            Hit::Tab(0)
        );
        assert_eq!(hit_test(&tabs, 799, 1), Hit::None);
    }

    #[test]
    fn reorder_target_follows_tab_midpoints() {
        let m = BarMetrics::default();
        let tabs = layout_tabs(2000, 3, &m);
        let mid = |i: usize| (tabs[i].tab.left + tabs[i].tab.right) / 2;
        assert_eq!(reorder_target(&tabs, 0, mid(1) + 1), 1);
        assert_eq!(reorder_target(&tabs, 0, mid(2) + 1), 2);
        assert_eq!(reorder_target(&tabs, 2, 0), 0);
        assert_eq!(reorder_target(&tabs, 1, 10_000), 2);
    }

    #[test]
    fn drop_zone_is_caption_band_or_bar() {
        let frame = Rect::new(100, 100, 500, 400);
        let bar = Rect::new(108, 68, 492, 100);
        assert!(in_drop_zone(frame, None, 32, 300, 110));
        assert!(!in_drop_zone(frame, None, 32, 300, 200));
        assert!(in_drop_zone(frame, Some(bar), 32, 300, 80));
        assert!(!in_drop_zone(frame, Some(bar), 32, 50, 80));
    }
}
