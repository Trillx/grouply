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
        [Self::Always, Self::Tucked, Self::Hidden]
            .into_iter()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
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
        out.push_str("# Grouply settings. Edit with any text editor, then choose\n");
        out.push_str("# \"Reload settings\" from the tray menu.\n\n");
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
        let m = &self.metrics;
        for (key, value) in [
            ("height", m.height),
            ("top_gap", m.top_gap),
            ("inset", m.inset),
            ("tab_min", m.tab_min),
            ("tab_max", m.tab_max),
            ("spacing", m.spacing),
            ("padding", m.padding),
            ("icon", m.icon),
            ("close", m.close),
            ("caption_reserve", m.caption_reserve),
            ("corner", m.corner),
        ] {
            let _ = writeln!(out, "{key} = {value}");
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
            ("", "hold_to_group_ms") => set_u64(&mut settings.hold_to_group_ms, value, 0, 5_000),
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
            ("bar", "height") => set_i32(&mut settings.metrics.height, value, 16, 96),
            ("bar", "top_gap") => set_i32(&mut settings.metrics.top_gap, value, 0, 32),
            ("bar", "inset") => set_i32(&mut settings.metrics.inset, value, 0, 64),
            ("bar", "tab_min") => set_i32(&mut settings.metrics.tab_min, value, 32, 400),
            ("bar", "tab_max") => set_i32(&mut settings.metrics.tab_max, value, 32, 800),
            ("bar", "spacing") => set_i32(&mut settings.metrics.spacing, value, 0, 16),
            ("bar", "padding") => set_i32(&mut settings.metrics.padding, value, 0, 32),
            ("bar", "icon") => set_i32(&mut settings.metrics.icon, value, 8, 64),
            ("bar", "close") => set_i32(&mut settings.metrics.close, value, 8, 64),
            ("bar", "caption_reserve") => {
                set_i32(&mut settings.metrics.caption_reserve, value, 0, 600)
            }
            ("bar", "corner") => set_i32(&mut settings.metrics.corner, value, 0, 32),
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

/// Stores a parsed integer, clamped to `[min, max]`. Returns false when `value` is not an integer.
fn set_i32(target: &mut i32, value: &str, min: i32, max: i32) -> bool {
    match value.parse::<i32>() {
        Ok(parsed) => {
            *target = parsed.clamp(min, max);
            true
        }
        Err(_) => false,
    }
}

fn set_u64(target: &mut u64, value: &str, min: u64, max: u64) -> bool {
    match value.parse::<u64>() {
        Ok(parsed) => {
            *target = parsed.clamp(min, max);
            true
        }
        Err(_) => false,
    }
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
