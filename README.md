<img src="assets/icon.svg" width="64" height="64" alt="">

# Grouply

Tabbed window groups for Windows: drag one window onto another's title bar and they become tabs in a shared frame. Built ARM64-first as a single native process, with no per-architecture helpers or injected DLLs.

Created in [T3 Code](https://t3.codes).

[![Build](https://github.com/Trillx/grouply/actions/workflows/build.yml/badge.svg)](https://github.com/Trillx/grouply/actions/workflows/build.yml)

## Status

Early prototype. The core model, geometry, settings and update logic are unit-tested, and CI builds every commit for ARM64 and x64. It **has not yet been tried on a real Windows desktop**, so expect rough edges.

## Download

Every commit to `main` is built and published as a release:

- [grouply-arm64.exe](https://github.com/Trillx/grouply/releases/latest/download/grouply-arm64.exe) for Windows on ARM
- [grouply-x64.exe](https://github.com/Trillx/grouply/releases/latest/download/grouply-x64.exe) for Intel and AMD PCs

There's no installer: put the exe in a folder you own (for example `%LOCALAPPDATA%\Grouply`) and run it. The builds aren't code-signed, so SmartScreen will warn the first time (*More info* → *Run anyway*).

## Using it

- **Group**: drag a window by its title bar over another window's title bar (or its tab bar). Hold for about ⅓ second, then release.
- **Switch tabs**: click a tab, or press <kbd>Win</kbd>+<kbd>`</kbd> (add <kbd>Shift</kbd> to go back).
- **Reorder**: drag a tab sideways. **Detach**: drag a tab well away from the bar.
- **Close**: the tab's × or a middle-click. Right-click a tab for more.
- **Move the group**: drag the active window; the rest follow.
- **Settings**: tray icon → *Settings file…* opens `%APPDATA%\Grouply\settings.toml`; *Reload settings* applies your edits without a restart.
- **Start with Windows**: tray icon → *Start with Windows* adds or removes the per-user sign-in entry.
- **Exit**: tray icon → *Exit Grouply*. Grouped windows are never hidden, so nothing is lost if Grouply stops.

## Settings

The first start writes `%APPDATA%\Grouply\settings.toml`. Edit it with any text editor, then choose *Reload settings* from the tray menu. Unknown keys are ignored and reported in a tray notification; numbers are clamped to safe ranges.

```toml
# Milliseconds a window must rest on a drop zone before it groups.
hold_to_group_ms = 350
middle_click_closes = true
cycle_hotkeys = true
# Tabs on maximized windows: "always" shows the whole bar, "tucked" leaves
# a sliver of each tab at the top edge, "hidden" shows nothing. Touching the
# top edge shows the whole bar.
maximized = "tucked"

# Bar dimensions in pixels at 96 DPI; scaled on other displays.
[bar]
height = 32
top_gap = 4
inset = 8
tab_min = 72
tab_max = 220
spacing = 2
padding = 8
icon = 16
close = 16
caption_reserve = 150
corner = 8
```

## Updates

Grouply checks for a newer build 30 seconds after it starts and every 6 hours after that. The tray icon's tooltip and menu show how many commits behind `main` you are. When a new build appears, a notification offers to update. Click it, or choose *Update and restart* from the tray menu. You can also run *Check for updates* there.

An update downloads the new exe, checks its SHA-256 and processor type, and swaps it in beside the running one. Then it restarts Grouply with your current groups intact. Builds you compile yourself report "development build" and never update.

How it works: CI numbers each build with its commit count (`git rev-list --count HEAD`), so two build numbers differ by exactly the number of commits between them. Each release carries a `manifest.txt` with the build number, commit and download hashes.

## Building

On a Windows on ARM device, install [Rust](https://rustup.rs) and the Visual Studio Build Tools with the *C++ ARM64 build tools* component, then:

```
cargo build --release
```

From another machine:

```
rustup target add aarch64-pc-windows-msvc
cargo build --release --target aarch64-pc-windows-msvc
```

Linking needs Microsoft's CRT libraries. On Linux or macOS, [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) can download them (you must accept Microsoft's license). Nothing in the code is ARM-specific; CI builds `x86_64-pc-windows-msvc` as well.

The platform-independent core tests run anywhere: `cargo test`.

## Layout

| Path | What |
| --- | --- |
| `src/model.rs` | Groups, membership, active tab, merge and removal rules |
| `src/layout.rs` | Bar placement, tab layout, hit testing, drop zones |
| `src/drag.rs` | Hold-to-group drop targeting |
| `src/command.rs` | Tab bar → controller commands |
| `src/update.rs` | Build identity, release manifest, commits-behind and update state |
| `src/handoff.rs` | Passing groups to the restarted instance after an update |
| `src/settings.rs` | Settings defaults, file parsing and writing |
| `src/platform/` | Win32: WinEvent hooks, controller, tab bar window, tray, autostart, updater, window helpers |
| `.github/workflows/build.yml` | CI: lint, test, build both architectures, publish releases from `main` |
| `docs/design.md` | Architecture and roadmap |

## Known limitations

- Windows of elevated apps can't be grouped unless Grouply itself runs elevated.
- On maximized windows the whole bar covers the top of the window, so by default it stays tucked to a sliver until the pointer touches the top edge. That reveal is polled every 100 ms rather than driven by a top-edge hook.
- Settings are file-only: there is no settings window, and some changes (like `cycle_hotkeys`) take effect after *Reload settings*, not while you edit the file.
- *Start with Windows* writes the standard per-user `Run` entry. It won't start Grouply elevated, and it points at wherever the exe lives now, so move the exe before enabling it.
- Tabs are drawn beside the title bar, not inside it. Integrating them into the title bar needs in-process code (see `docs/design.md`).
- Updating only works from a folder Grouply can write to, not from `Program Files`.

## License

[MIT](LICENSE)
