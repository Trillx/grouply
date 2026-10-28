//! User-tunable behaviour and its on-disk form.
//!
//! Settings live in a small TOML file under `%APPDATA%\Grouply`. Any text editor can change them;
//! *Reload settings* in the tray menu applies the file without restarting. Only the keys below are
//! read: unknown lines and sections are ignored with a warning, so a newer or older file still
//! loads. Values are clamped to sensible ranges rather than rejected.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::layout::BarMetrics;

/// Everything the user can change. The defaults are the built-in behaviour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// How long a dragged window must hover over a drop zone before releasing it groups.
    pub hold_to_group_ms: u64,
    /// Middle-clicking a tab closes that window.
    pub middle_click_closes: bool,
    /// Win+` / Win+Shift+` cycle tabs in the foreground group.
    pub cycle_hotkeys: bool,
    /// What the bar does on maximized (and top-snapped) windows.
    pub maximized: MaximizedBar,
    /// Bar dimensions at 96 DPI.
    pub metrics: BarMetrics,
}

/// The bar's behaviour on maximized and top-snapped windows. With no room above them, the bar can
/// only cover the top of the window, where the window keeps its own title, tabs or menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaximizedBar {
    /// The full bar always covers the top of the window.
    Always,
    /// A thin sliver of each tab stays at the top edge; touching the edge shows the full bar.
    Tucked,
    /// Nothing shows until the pointer touches the top edge.
    Hidden,
}

impl MaximizedBar {
    fn name(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Tucked => "tucked",
            Self::Hidden => "hidden",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
    }

    pub const ALL: [Self; 3] = [Self::Always, Self::Tucked, Self::Hidden];
}

/// A numeric setting: its key in the file and the range its values are clamped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    HoldToGroup,
    Height,
    TopGap,
    Inset,
    TabMin,
    TabMax,
    Spacing,
    Padding,
    Icon,
    Close,
    CaptionReserve,
    Corner,
}

impl Field {
    /// The `[bar]` fields, in the order the file lists them.
    pub const BAR: [Self; 11] = [
        Self::Height,
        Self::TopGap,
        Self::Inset,
        Self::TabMin,
        Self::TabMax,
        Self::Spacing,
        Self::Padding,
        Self::Icon,
        Self::Close,
        Self::CaptionReserve,
        Self::Corner,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::HoldToGroup => "hold_to_group_ms",
            Self::Height => "height",
            Self::TopGap => "top_gap",
            Self::Inset => "inset",
            Self::TabMin => "tab_min",
            Self::TabMax => "tab_max",
            Self::Spacing => "spacing",
            Self::Padding => "padding",
            Self::Icon => "icon",
            Self::Close => "close",
            Self::CaptionReserve => "caption_reserve",
            Self::Corner => "corner",
        }
    }

    /// The file section holding the key: `""` for the top level.
    fn section(self) -> &'static str {
        if self == Self::HoldToGroup { "" } else { "bar" }
    }

    /// Inclusive range of accepted values.
    pub fn range(self) -> (i32, i32) {
        match self {
            Self::HoldToGroup => (0, 5_000),
            Self::Height => (16, 96),
            Self::TopGap => (0, 32),
            Self::Inset => (0, 64),
            Self::TabMin => (32, 400),
            Self::TabMax => (32, 800),
            Self::Spacing => (0, 16),
            Self::Padding => (0, 32),
            Self::Icon | Self::Close => (8, 64),
            Self::CaptionReserve => (0, 600),
            Self::Corner => (0, 32),
        }
    }

    pub fn get(self, s: &Settings) -> i32 {
        let m = &s.metrics;
        match self {
            Self::HoldToGroup => s.hold_to_group_ms as i32,
            Self::Height => m.height,
            Self::TopGap => m.top_gap,
            Self::Inset => m.inset,
            Self::TabMin => m.tab_min,
            Self::TabMax => m.tab_max,
            Self::Spacing => m.spacing,
            Self::Padding => m.padding,
            Self::Icon => m.icon,
            Self::Close => m.close,
            Self::CaptionReserve => m.caption_reserve,
            Self::Corner => m.corner,
        }
    }

    /// Stores `value` clamped to the range. Raising the narrowest tab past the widest raises
    /// the widest too, and lowering the widest below the narrowest lowers the narrowest.
    pub fn set(self, s: &mut Settings, value: i32) {
        self.store(s, value.into());
        let m = &mut s.metrics;
        match self {
            Self::TabMin if m.tab_max < m.tab_min => m.tab_max = m.tab_min,
            // Both ranges start at 32, and tab_min tops out at 400, so this stays in range.
            Self::TabMax if m.tab_min > m.tab_max => m.tab_min = m.tab_max,
            _ => {}
        }
    }

    /// Stores `value` clamped to the range, leaving other fields alone.
    fn store(self, s: &mut Settings, value: i64) {
        let (min, max) = self.range();
        let v = value.clamp(min.into(), max.into()) as i32;
        let m = &mut s.metrics;
        match self {
            Self::HoldToGroup => s.hold_to_group_ms = v as u64,
            Self::Height => m.height = v,
            Self::TopGap => m.top_gap = v,
            Self::Inset => m.inset = v,
            Self::TabMin => m.tab_min = v,
            Self::TabMax => m.tab_max = v,
            Self::Spacing => m.spacing = v,
            Self::Padding => m.padding = v,
            Self::Icon => m.icon = v,
            Self::Close => m.close = v,
            Self::CaptionReserve => m.caption_reserve = v,
            Self::Corner => m.corner = v,
        }
    }

    fn find(section: &str, key: &str) -> Option<Self> {
        std::iter::once(Self::HoldToGroup)
            .chain(Self::BAR)
            .find(|f| f.section() == section && f.key() == key)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hold_to_group_ms: 350,
            middle_click_closes: true,
            cycle_hotkeys: true,
            maximized: MaximizedBar::Tucked,
            metrics: BarMetrics::default(),
        }
    }
}

/// The result of reading a settings file: always usable settings, plus anything that looked wrong.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    pub warnings: Vec<String>,
}

/// `%APPDATA%\Grouply\settings.toml`, falling back to `%LOCALAPPDATA%`.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("LOCALAPPDATA"))?;
    Some(PathBuf::from(base).join("Grouply").join("settings.toml"))
}

/// Reads `path`, or writes and returns the defaults when it does not exist yet.
pub fn load_or_create(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let settings = Settings::default();
            let mut loaded = Loaded {
                settings,
                warnings: Vec::new(),
            };
            if let Err(error) = loaded.settings.save(path) {
                loaded
                    .warnings
                    .push(format!("couldn't create {}: {error}", path.display()));
            }
            loaded
        }
        Err(error) => Loaded {
            warnings: vec![format!("couldn't read {}: {error}", path.display())],
            ..Loaded::default()
        },
    }
}

/// Reads `path` for an explicit reload. Missing or unreadable files fall back to the defaults.
pub fn load(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) => Loaded {
            warnings: vec![format!("couldn't read {}: {error}", path.display())],
            ..Loaded::default()
        },
    }
}

impl Settings {
    /// Writes the settings, creating the containing folder when needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.to_toml())
    }

    /// The file this `save` writes, with a short explanation of every key.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# Grouply settings. Change them in the settings window (tray icon,\n");
        out.push_str("# Settings), or edit this file and choose \"Reload settings\" from the\n");
        out.push_str("# tray menu. The settings window rewrites this file.\n\n");
        out.push_str("# Milliseconds a window must rest on a drop zone before it groups.\n");
        let _ = writeln!(out, "hold_to_group_ms = {}", self.hold_to_group_ms);
        let _ = writeln!(out, "middle_click_closes = {}", self.middle_click_closes);
        let _ = writeln!(out, "cycle_hotkeys = {}", self.cycle_hotkeys);
        out.push_str(
            "# Tabs on maximized windows: \"always\" shows the whole bar, \"tucked\" leaves\n",
        );
        out.push_str(
            "# a sliver of each tab at the top edge, \"hidden\" shows nothing. Touching the\n",
        );
        out.push_str("# top edge shows the whole bar.\n");
        let _ = writeln!(out, "maximized = \"{}\"", self.maximized.name());
        out.push_str("\n# Bar dimensions in pixels at 96 DPI; scaled on other displays.\n");
        out.push_str("[bar]\n");
        for field in Field::BAR {
            let _ = writeln!(out, "{} = {}", field.key(), field.get(self));
        }
        out
    }
}

/// Parses the file, never failing: bad lines keep the previous value and add a warning.
pub fn parse(text: &str) -> Loaded {
    let mut settings = Settings::default();
    let mut warnings = Vec::new();
    let mut section = String::new();

    for (index, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let number = index + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            if section != "bar" {
                warnings.push(format!("line {number}: unknown section [{section}]"));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("line {number}: expected key = value"));
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let known = match (section.as_str(), key.as_str()) {
            ("", "middle_click_closes") => set_bool(&mut settings.middle_click_closes, value),
            ("", "cycle_hotkeys") => set_bool(&mut settings.cycle_hotkeys, value),
            ("", "maximized") => set_maximized(&mut settings.maximized, value),
            // Older files: true hid the bar, false kept it on.
            ("", "hide_when_maximized") => {
                let mut hide = false;
                let known = set_bool(&mut hide, value);
                if known {
                    settings.maximized = if hide {
                        MaximizedBar::Hidden
                    } else {
                        MaximizedBar::Always
                    };
                }
                known
            }
            (section, key) if let Some(field) = Field::find(section, key) => {
                match value.parse::<i64>() {
                    Ok(parsed) => {
                        field.store(&mut settings, parsed);
                        true
                    }
                    Err(_) => false,
                }
            }
            _ => {
                let where_ = if section.is_empty() {
                    String::new()
                } else {
                    format!(" in [{section}]")
                };
                warnings.push(format!("line {number}: unknown key `{key}`{where_}"));
                true
            }
        };
        if !known {
            let expected = if key == "maximized" {
                "\"always\", \"tucked\" or \"hidden\""
            } else {
                "a number or true/false"
            };
            warnings.push(format!("line {number}: `{key}` is not {expected}"));
        }
    }

    // A maximum below the minimum would make the layout panic on its clamp.
    if settings.metrics.tab_max < settings.metrics.tab_min {
        settings.metrics.tab_max = settings.metrics.tab_min;
    }
    Loaded { settings, warnings }
}

/// Stores a mode name, quoted or not. Returns false when it isn't one.
fn set_maximized(target: &mut MaximizedBar, value: &str) -> bool {
    let name = value.trim_matches(|c| c == '"' || c == '\'');
    match MaximizedBar::from_name(name) {
        Some(mode) => {
            *target = mode;
            true
        }
        None => false,
    }
}

fn set_bool(target: &mut bool, value: &str) -> bool {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => {
            *target = true;
            true
        }
        "false" | "no" | "off" | "0" => {
            *target = false;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> Settings {
        Settings {
            hold_to_group_ms: 666,
            middle_click_closes: false,
            cycle_hotkeys: false,
            maximized: MaximizedBar::Hidden,
            metrics: BarMetrics {
                height: 40,
                top_gap: 6,
                inset: 12,
                tab_min: 90,
                tab_max: 260,
                spacing: 4,
                padding: 10,
                icon: 20,
                close: 18,
                caption_reserve: 180,
                corner: 10,
            },
        }
    }

    #[test]
    fn defaults_round_trip_through_the_file() {
        let text = Settings::default().to_toml();
        let loaded = parse(&text);
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn a_custom_file_round_trips() {
        let settings = custom();
        let loaded = parse(&settings.to_toml());
        assert_eq!(loaded.settings, settings);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn comments_blank_lines_and_unknown_keys_are_tolerated() {
        let loaded = parse(
            "\u{feff}# heading\n\n hold_to_group_ms = 500 # trailing\n[bar]\nheight = 36\n[future]\nx = 1\nmystery = 2\n",
        );
        assert_eq!(loaded.settings.hold_to_group_ms, 500);
        assert_eq!(loaded.settings.metrics.height, 36);
        assert_eq!(loaded.warnings.len(), 3, "{:?}", loaded.warnings);
    }

    #[test]
    fn bad_values_keep_the_default_and_warn() {
        let loaded = parse("hold_to_group_ms = soon\nmiddle_click_closes = maybe\n");
        assert_eq!(loaded.settings.hold_to_group_ms, 350);
        assert!(loaded.settings.middle_click_closes);
        assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
    }

    #[test]
    fn values_are_clamped_to_sane_ranges() {
        let loaded = parse(
            "hold_to_group_ms = 999999\n[bar]\nheight = 1\ntab_min = 2\ntab_max = 3\nspacing = 99\n",
        );
        assert_eq!(loaded.settings.hold_to_group_ms, 5_000);
        assert_eq!(loaded.settings.metrics.height, 16);
        assert_eq!(loaded.settings.metrics.tab_min, 32);
        assert_eq!(loaded.settings.metrics.tab_max, 32);
        assert_eq!(loaded.settings.metrics.spacing, 16);
    }

    #[test]
    fn yes_no_and_numbers_are_accepted_for_flags() {
        let loaded = parse("middle_click_closes = no\ncycle_hotkeys = 0\n");
        assert!(!loaded.settings.middle_click_closes);
        assert!(!loaded.settings.cycle_hotkeys);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn maximized_modes_parse_quoted_or_bare_in_any_case() {
        for (text, mode) in [
            ("maximized = \"always\"", MaximizedBar::Always),
            ("maximized = 'Hidden'", MaximizedBar::Hidden),
            ("maximized = TUCKED", MaximizedBar::Tucked),
        ] {
            let loaded = parse(text);
            assert_eq!(loaded.settings.maximized, mode, "{text}");
            assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        }
        let loaded = parse("maximized = \"folded\"");
        assert_eq!(loaded.settings.maximized, MaximizedBar::Tucked);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    }

    #[test]
    fn the_old_hide_when_maximized_key_keeps_its_meaning() {
        let hidden = parse("hide_when_maximized = YES\n");
        assert_eq!(hidden.settings.maximized, MaximizedBar::Hidden);
        let shown = parse("hide_when_maximized = false\n");
        assert_eq!(shown.settings.maximized, MaximizedBar::Always);
        assert!(shown.warnings.is_empty(), "{:?}", shown.warnings);
    }

    #[test]
    fn fields_read_and_write_the_settings_they_name() {
        let mut s = Settings::default();
        for (i, field) in std::iter::once(Field::HoldToGroup)
            .chain(Field::BAR)
            .enumerate()
        {
            let (min, max) = field.range();
            let value = (min + 1 + i as i32).min(max);
            field.set(&mut s, value);
            assert_eq!(field.get(&s), value, "{field:?}");
            assert_eq!(Field::find(field.section(), field.key()), Some(field));
        }
        Field::Height.set(&mut s, 1_000);
        assert_eq!(s.metrics.height, 96);
        Field::HoldToGroup.set(&mut s, -5);
        assert_eq!(s.hold_to_group_ms, 0);
    }

    #[test]
    fn tab_limits_push_each_other_instead_of_crossing() {
        let mut s = Settings::default();
        Field::TabMin.set(&mut s, 300);
        assert_eq!((s.metrics.tab_min, s.metrics.tab_max), (300, 300));
        Field::TabMax.set(&mut s, 100);
        assert_eq!((s.metrics.tab_min, s.metrics.tab_max), (100, 100));
        Field::TabMax.set(&mut s, 500);
        assert_eq!((s.metrics.tab_min, s.metrics.tab_max), (100, 500));
    }

    /// A fresh, legal-on-Windows directory name for the two filesystem tests.
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!(
            "grouply-settings-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn save_and_load_use_the_same_format() {
        let dir = temp_dir("round-trip");
        let path = dir.join("settings.toml");
        custom().save(&path).expect("write settings");
        let loaded = load(&path);
        assert_eq!(loaded.settings, custom());
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_created_with_the_defaults() {
        let dir = temp_dir("create");
        let path = dir.join("nested").join("settings.toml");
        let loaded = load_or_create(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert!(path.exists());
        assert_eq!(load(&path).settings, Settings::default());
        let _ = fs::remove_dir_all(&dir);
    }
}
