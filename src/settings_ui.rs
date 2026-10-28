//! Platform-independent half of the settings window: presets, dial arithmetic, and where every
//! control goes.
//!
//! The window has a pinned header holding a live preview of the tab bar, and a scrolling page
//! below it: presets, dials for the most-used sizes, behaviour toggles, the maximized-window
//! mode, and a collapsible list of the remaining sizes. Rectangles are in physical pixels and in
//! page coordinates, i.e. as if the page were not scrolled; the header never scrolls.

use crate::layout::{BarMetrics, Rect};
use crate::settings::{Field, MaximizedBar};

/// A starting point for the bar's look. Applying one leaves the inset and the room kept for
/// caption buttons alone, since those depend on the user's apps rather than on taste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    height: i32,
    top_gap: i32,
    tab_min: i32,
    tab_max: i32,
    spacing: i32,
    padding: i32,
    icon: i32,
    close: i32,
    corner: i32,
}

pub const PRESETS: [Preset; 5] = [
    Preset {
        name: "Compact",
        height: 26,
        top_gap: 2,
        tab_min: 64,
        tab_max: 180,
        spacing: 2,
        padding: 6,
        icon: 14,
        close: 14,
        corner: 6,
    },
    Preset {
        name: "Standard",
        height: 32,
        top_gap: 4,
        tab_min: 72,
        tab_max: 220,
        spacing: 2,
        padding: 8,
        icon: 16,
        close: 16,
        corner: 8,
    },
    Preset {
        name: "Roomy",
        height: 40,
        top_gap: 6,
        tab_min: 96,
        tab_max: 260,
        spacing: 4,
        padding: 12,
        icon: 18,
        close: 18,
        corner: 10,
    },
    Preset {
        name: "Rounded",
        height: 32,
        top_gap: 4,
        tab_min: 72,
        tab_max: 220,
        spacing: 6,
        padding: 10,
        icon: 16,
        close: 16,
        corner: 24,
    },
    Preset {
        name: "Flat",
        height: 30,
        top_gap: 0,
        tab_min: 72,
        tab_max: 220,
        spacing: 0,
        padding: 8,
        icon: 16,
        close: 16,
        corner: 0,
    },
];

impl Preset {
    pub fn apply(&self, m: BarMetrics) -> BarMetrics {
        BarMetrics {
            height: self.height,
            top_gap: self.top_gap,
            tab_min: self.tab_min,
            tab_max: self.tab_max,
            spacing: self.spacing,
            padding: self.padding,
            icon: self.icon,
            close: self.close,
            corner: self.corner,
            ..m
        }
    }

    pub fn matches(&self, m: &BarMetrics) -> bool {
        self.apply(*m) == *m
    }
}

/// The sizes with a dial, left to right.
pub const DIALS: [Field; 5] = [
    Field::Height,
    Field::TabMax,
    Field::Corner,
    Field::Spacing,
    Field::HoldToGroup,
];

/// The sizes under "More sizes", in reading order (two columns).
pub const SLIDERS: [Field; 7] = [
    Field::TopGap,
    Field::Inset,
    Field::TabMin,
    Field::Padding,
    Field::Icon,
    Field::Close,
    Field::CaptionReserve,
];

/// What the window calls a size.
pub fn label(field: Field) -> &'static str {
    match field {
        Field::HoldToGroup => "Hold to group",
        Field::Height => "Height",
        Field::TopGap => "Space above tabs",
        Field::Inset => "Left inset",
        Field::TabMin => "Narrowest tab",
        Field::TabMax => "Widest tab",
        Field::Spacing => "Gap",
        Field::Padding => "Padding inside tabs",
        Field::Icon => "Icon size",
        Field::Close => "Close button",
        Field::CaptionReserve => "Room for caption buttons",
        Field::Corner => "Roundness",
    }
}

pub fn unit(field: Field) -> &'static str {
    if field == Field::HoldToGroup {
        "ms"
    } else {
        "px"
    }
}

/// Values a dial or slider spans. Holding longer than two seconds is allowed in the file, but
/// isn't worth a quarter of the dial.
pub fn control_range(field: Field) -> (i32, i32) {
    match field {
        Field::HoldToGroup => (0, 2_000),
        _ => field.range(),
    }
}

/// How far round the dial (or along the slider) `value` sits, from 0 to 1.
pub fn fraction(field: Field, value: i32) -> f32 {
    let (min, max) = control_range(field);
    ((value.clamp(min, max) - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
}

/// The value at `fraction` of the way along a slider.
pub fn value_at(field: Field, fraction: f32) -> i32 {
    let (min, max) = control_range(field);
    let v = min as f32 + fraction.clamp(0.0, 1.0) * (max - min) as f32;
    snap(field, v.round() as i32)
}

/// Dragging a dial this many DIPs turns it through its whole range.
pub const DIAL_TRAVEL: i32 = 200;

/// A dial's value after dragging `dy` physical pixels down (negative is up) from `start`.
pub fn dial_drag(field: Field, start: i32, dy: i32, dpi: u32) -> i32 {
    let (min, max) = control_range(field);
    let travel = (DIAL_TRAVEL * dpi.max(1) as i32 / 96).max(1) as f32;
    let v = start as f32 - dy as f32 * (max - min) as f32 / travel;
    snap(field, v.round() as i32).clamp(min.min(start), max.max(start))
}

/// One arrow-key or mouse-wheel step; Shift or Page Up/Down moves ten.
pub fn step(field: Field) -> i32 {
    match field {
        Field::HoldToGroup => 25,
        Field::TabMin | Field::TabMax | Field::CaptionReserve => 4,
        _ => 1,
    }
}

/// Rounds hold times to the 25 ms the controls step in; sizes are whole pixels already.
fn snap(field: Field, value: i32) -> i32 {
    let unit = if field == Field::HoldToGroup { 25 } else { 1 };
    (value + unit / 2).div_euclid(unit) * unit
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    MiddleClick,
    CycleHotkeys,
    Autostart,
}

pub const TOGGLES: [Toggle; 3] = [Toggle::MiddleClick, Toggle::CycleHotkeys, Toggle::Autostart];

/// Everything that takes a click or keyboard focus, in focus order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Preset(usize),
    Dial(Field),
    Toggle(Toggle),
    Mode(MaximizedBar),
    /// Shows or hides the remaining sizes.
    More,
    Slider(Field),
    Reset,
    OpenFile,
}

/// Section headings, in page order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Presets,
    FineTune,
    Behaviour,
    Maximized,
}

/// Default client width in DIPs.
pub const WIDTH: i32 = 600;

/// Positions of everything in the window at one DPI and width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Height of the pinned header above the scrolling page.
    pub header: i32,
    pub preview: Rect,
    pub sections: Vec<(Section, Rect)>,
    pub controls: Vec<(Control, Rect)>,
    /// Height of the whole page, header included, when not scrolled.
    pub height: i32,
    /// Room kept above a control scrolled into view, enough for its section heading.
    pad: i32,
}

impl Layout {
    /// Lays out a window `width` physical pixels wide. `more` shows the remaining sizes.
    pub fn new(width: i32, dpi: u32, more: bool) -> Self {
        let s = |v: i32| v * dpi.max(1) as i32 / 96;
        let margin = s(20);
        let gap = s(8);
        let left = margin;
        let right = (width - margin).max(left + s(200));
        let inner = right - left;
        let columns = |n: i32, i: i32| {
            let w = (inner - gap * (n - 1)) / n;
            let l = left + i * (w + gap);
            (l, l + w)
        };

        let preview = Rect::new(left, s(16), right, s(16) + s(140));
        let header = preview.bottom + s(12);
        let mut y = header + s(4);
        let mut sections = Vec::new();
        let mut controls = Vec::new();
        let heading = |sections: &mut Vec<(Section, Rect)>, y: &mut i32, section| {
            sections.push((section, Rect::new(left, *y, right, *y + s(20))));
            *y += s(28);
        };

        heading(&mut sections, &mut y, Section::Presets);
        for i in 0..PRESETS.len() {
            let (l, r) = columns(PRESETS.len() as i32, i as i32);
            controls.push((Control::Preset(i), Rect::new(l, y, r, y + s(76))));
        }
        y += s(76) + s(24);

        heading(&mut sections, &mut y, Section::FineTune);
        for (i, &field) in DIALS.iter().enumerate() {
            let (l, r) = columns(DIALS.len() as i32, i as i32);
            controls.push((Control::Dial(field), Rect::new(l, y, r, y + s(108))));
        }
        y += s(108) + s(20);

        heading(&mut sections, &mut y, Section::Behaviour);
        for (i, &toggle) in TOGGLES.iter().enumerate() {
            let (l, r) = columns(TOGGLES.len() as i32, i as i32);
            controls.push((Control::Toggle(toggle), Rect::new(l, y, r, y + s(56))));
        }
        y += s(56) + s(24);

        heading(&mut sections, &mut y, Section::Maximized);
        for (i, &mode) in MaximizedBar::ALL.iter().enumerate() {
            let (l, r) = columns(MaximizedBar::ALL.len() as i32, i as i32);
            controls.push((Control::Mode(mode), Rect::new(l, y, r, y + s(72))));
        }
        y += s(72) + s(20);

        controls.push((Control::More, Rect::new(left, y, right, y + s(32))));
        y += s(32) + s(8);
        if more {
            let rows = SLIDERS.len().div_ceil(2) as i32;
            for (i, &field) in SLIDERS.iter().enumerate() {
                let (l, r) = columns(2, i as i32 % 2);
                let top = y + (i as i32 / 2) * s(48);
                controls.push((Control::Slider(field), Rect::new(l, top, r, top + s(44))));
            }
            y += rows * s(48) + s(8);
        }
        y += s(8);

        let button = |l: i32, w: i32| Rect::new(l, y, l + w, y + s(32));
        controls.push((Control::Reset, button(left, s(148))));
        controls.push((Control::OpenFile, button(left + s(148) + gap, s(148))));
        y += s(32) + margin;

        Self {
            header,
            preview,
            sections,
            controls,
            height: y,
            pad: s(32),
        }
    }

    pub fn rect(&self, control: Control) -> Option<Rect> {
        self.controls
            .iter()
            .find(|(c, _)| *c == control)
            .map(|&(_, r)| r)
    }

    /// The furthest the page can scroll in a client area `client_height` pixels tall.
    pub fn max_scroll(&self, client_height: i32) -> i32 {
        (self.height - client_height).max(0)
    }

    /// The control under client point (x, y). The header covers the page as it scrolls under it.
    pub fn hit(&self, x: i32, y: i32, scroll: i32) -> Option<Control> {
        if y < self.header {
            return None;
        }
        self.controls
            .iter()
            .find(|(_, r)| r.contains(x, y + scroll))
            .map(|&(c, _)| c)
    }

    /// The next control in focus order after `from` (or before it, `backwards`), wrapping round.
    pub fn next_focus(&self, from: Option<Control>, backwards: bool) -> Option<Control> {
        let n = self.controls.len();
        if n == 0 {
            return None;
        }
        let at = from.and_then(|c| self.controls.iter().position(|(x, _)| *x == c));
        let i = match (at, backwards) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        Some(self.controls[i].0)
    }

    /// The scroll offset that brings `control` fully into view below the header, moving as
    /// little as possible.
    pub fn scroll_to(&self, control: Control, scroll: i32, client_height: i32) -> i32 {
        let Some(r) = self.rect(control) else {
            return scroll;
        };
        let scroll = if r.top - scroll < self.header {
            r.top - self.header - self.pad
        } else if r.bottom - scroll > client_height {
            r.bottom - client_height + self.pad / 2
        } else {
            scroll
        };
        scroll.clamp(0, self.max_scroll(client_height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn standard_is_the_default_look_and_presets_differ() {
        let defaults = BarMetrics::default();
        let standard = PRESETS.iter().find(|p| p.name == "Standard").unwrap();
        assert!(standard.matches(&defaults));
        for (i, a) in PRESETS.iter().enumerate() {
            assert!(a.matches(&a.apply(defaults)), "{}", a.name);
            for b in &PRESETS[i + 1..] {
                assert!(!b.matches(&a.apply(defaults)), "{} vs {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn presets_stay_inside_the_file_limits_and_keep_inset_and_reserve() {
        let m = BarMetrics {
            inset: 20,
            caption_reserve: 300,
            ..BarMetrics::default()
        };
        for preset in &PRESETS {
            let mut s = Settings {
                metrics: preset.apply(m),
                ..Settings::default()
            };
            assert_eq!((s.metrics.inset, s.metrics.caption_reserve), (20, 300));
            for field in Field::BAR {
                let before = field.get(&s);
                field.set(&mut s, before);
                assert_eq!(field.get(&s), before, "{} {field:?}", preset.name);
            }
        }
    }

    #[test]
    fn dials_map_values_to_fractions_and_drags_to_values() {
        assert_eq!(fraction(Field::Height, 16), 0.0);
        assert_eq!(fraction(Field::Height, 96), 1.0);
        assert_eq!(fraction(Field::HoldToGroup, 4_000), 1.0, "beyond the dial");
        // A full upward drag of the dial's travel spans the range.
        assert_eq!(dial_drag(Field::Height, 16, -DIAL_TRAVEL, 96), 96);
        assert_eq!(dial_drag(Field::Height, 16, -2 * DIAL_TRAVEL, 192), 96);
        assert_eq!(dial_drag(Field::Height, 40, 0, 96), 40);
        assert_eq!(dial_drag(Field::Height, 40, 10_000, 96), 16);
        // Hold times move in 25 ms steps, and a value past the dial isn't pulled back.
        assert_eq!(dial_drag(Field::HoldToGroup, 350, -3, 96) % 25, 0);
        assert_eq!(dial_drag(Field::HoldToGroup, 4_000, 0, 96), 4_000);
        assert_eq!(value_at(Field::Height, 0.5), 56);
        assert_eq!(value_at(Field::HoldToGroup, 0.1), 200);
    }

    #[test]
    fn controls_fit_the_width_and_never_overlap() {
        for (dpi, more) in [(96, false), (96, true), (144, true), (192, false)] {
            let width = WIDTH * dpi as i32 / 96;
            let layout = Layout::new(width, dpi, more);
            for (i, (a, ra)) in layout.controls.iter().enumerate() {
                assert!(ra.left >= 0 && ra.right <= width, "{a:?} at {dpi}");
                assert!(
                    ra.top >= layout.header && ra.bottom <= layout.height,
                    "{a:?}"
                );
                for (b, rb) in &layout.controls[i + 1..] {
                    let overlap = ra.left < rb.right
                        && rb.left < ra.right
                        && ra.top < rb.bottom
                        && rb.top < ra.bottom;
                    assert!(!overlap, "{a:?} overlaps {b:?} at {dpi}");
                }
            }
            let sliders = layout
                .controls
                .iter()
                .filter(|(c, _)| matches!(c, Control::Slider(_)))
                .count();
            assert_eq!(sliders, if more { SLIDERS.len() } else { 0 });
        }
        let closed = Layout::new(WIDTH, 96, false);
        let open = Layout::new(WIDTH, 96, true);
        assert!(open.height > closed.height);
    }

    #[test]
    fn hits_account_for_scrolling_and_the_pinned_header() {
        let layout = Layout::new(WIDTH, 96, false);
        let reset = layout.rect(Control::Reset).unwrap();
        let (x, y) = (
            (reset.left + reset.right) / 2,
            (reset.top + reset.bottom) / 2,
        );
        assert_eq!(layout.hit(x, y, 0), Some(Control::Reset));
        assert_eq!(layout.hit(x, y - 100, 100), Some(Control::Reset));
        // A preset scrolled up under the header can't be clicked through it.
        let preset = layout.rect(Control::Preset(0)).unwrap();
        let scroll = preset.bottom - layout.header + 1;
        assert_eq!(
            layout.hit(preset.left + 1, preset.top - scroll, scroll),
            None
        );
        assert_eq!(layout.hit(-1, -1, 0), None);
    }

    #[test]
    fn focus_cycles_and_scrolling_follows_it() {
        let layout = Layout::new(WIDTH, 96, false);
        let first = layout.next_focus(None, false);
        assert_eq!(first, Some(Control::Preset(0)));
        assert_eq!(layout.next_focus(first, true), Some(Control::OpenFile));
        assert_eq!(
            layout.next_focus(Some(Control::OpenFile), false),
            Some(Control::Preset(0))
        );

        let client = 400;
        let bottom = layout.scroll_to(Control::OpenFile, 0, client);
        let r = layout.rect(Control::OpenFile).unwrap();
        assert!(r.bottom - bottom <= client && r.top - bottom >= layout.header);
        assert_eq!(layout.scroll_to(Control::Preset(0), bottom, client), 0);
    }
}
