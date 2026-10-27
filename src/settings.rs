use crate::layout::BarMetrics;

/// User-tunable behaviour. Persistence is not implemented yet; these are the built-in defaults.
#[derive(Clone, Debug)]
pub struct Settings {
    /// How long a dragged window must hover over a drop zone before releasing it groups.
    pub hold_to_group_ms: u64,
    /// Middle-clicking a tab closes that window.
    pub middle_click_closes: bool,
    /// Win+` / Win+Shift+` cycle tabs in the foreground group.
    pub cycle_hotkeys: bool,
    /// Bar dimensions at 96 DPI.
    pub metrics: BarMetrics,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hold_to_group_ms: 350,
            middle_click_closes: true,
            cycle_hotkeys: true,
            metrics: BarMetrics::default(),
        }
    }
}
