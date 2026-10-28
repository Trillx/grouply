# Grouply design

## Goal

Window tabbing in the style of Stardock Groupy 2, starting with Windows on ARM. Grouply is a clean-room implementation built on documented Win32 APIs. It contains no code, assets or text from Groupy.

## Why the architecture is different on ARM

Groupy-style tools traditionally load a hook DLL into every GUI process with `SetWindowsHookEx`. A DLL must match its host process's architecture. On Windows on ARM that means three hook builds (ARM64, x64 for emulated apps, x86) and a helper process per architecture to install each one. Groupy 2 ships exactly that set of files.

Grouply's first milestone avoids injection altogether:

- **Events** come from `SetWinEventHook` with `WINEVENT_OUTOFCONTEXT`. The system delivers them to our own thread, from every process regardless of architecture.
- **Window management** (`SetWindowPos`, `ShowWindowAsync`, `SetForegroundWindow`, DWM frame queries) works across processes and architectures, because it is handled by the window manager, not the target's code.
- **Tabs** are our own layered popup window, kept directly above the group's active window in the z-order.

The result is one native ARM64 executable that handles native, x64-emulated, x86-emulated and ARM64EC apps the same way.

## Runtime model

```
WinEvent hook callback ──queue──▶ WM_APP_EVENTS ──▶ App (controller)
Tab bar mouse input   ──PostMessage(WM_APP_TAB)──▶ App
Timer (during drags)  ──WM_TIMER──────────────────▶ App
App ──▶ SetWindowPos / ShowWindowAsync on member windows (async, never blocks on hung apps)
App ──▶ TabBar::set_tabs / place
```

- Everything runs on one UI thread. Hook callbacks and tab bars never touch `App` directly. They queue or post, so the window procedures cannot re-enter `App`.
- **Inactive members are never hidden.** They sit at the active window's position, directly beneath it, so the taskbar, Alt+Tab and crash recovery all keep working. Moving or resizing the active window moves the rest (`sync`). Bringing any member forward makes it the active tab (`on_foreground`).
- **Grouping** happens on drag release, once the cursor has stayed for `hold_to_group_ms` over a target's caption band or tab bar (`DropTracker`). The merged group takes the target's frame.
- **Minimize** applies to the whole group. Grouply's own follow-up minimize and restore events are recognised and ignored (`minimized`, `restoring`).
- **Focus**: the bar is `WS_EX_NOACTIVATE`, so a tab click leaves Grouply holding the last input event. That makes the following `SetForegroundWindow` legitimate.
- **Coordinates** are physical pixels (per-monitor-v2 DPI awareness). The visible frame comes from `DWMWA_EXTENDED_FRAME_BOUNDS`, and each window keeps its own invisible-border insets when resized.
- Windows of more elevated processes are excluded; UIPI would reject our moves.
- **Settings** are read from `%APPDATA%\Grouply\settings.toml` at startup and from *Reload settings*; the file is plain TOML, unknown keys are reported and ignored, and values are clamped. **Start with Windows** is one `HKCU\...\CurrentVersion\Run` value, read back from the registry rather than mirrored in the file.
- **Maximized and top-snapped windows** leave no room above them, so the bar has to cover the window's own caption. `maximized` picks what it does there: `always` keeps the whole bar, `tucked` (the default) shrinks it to a click-through sliver of each tab along the top edge, and `hidden` removes it. A 100 ms cursor poll expands the bar while the pointer is at the top edge and collapses it when the pointer leaves, but never while a tab is pressed. The timer only runs when `maximized` isn't `always`. Files with the older `hide_when_maximized` key keep their meaning: `true` is `hidden`, `false` is `always`.

## Updates

```
CI (push to main) ──▶ release build-N: grouply-arm64.exe, grouply-x64.exe, manifest.txt
Timer / tray menu ──▶ worker thread: GET releases/latest/download/manifest.txt ──▶ WM_APP_UPDATE
Update click      ──▶ worker thread: download, SHA-256, PE machine check, write grouply.exe.new
                  ──▶ UI thread: grouply.exe → .old, .new → grouply.exe, start it --updated --groups=…, exit
New instance      ──▶ waits for the old instance's mutex to be abandoned, adopts the groups, deletes .old
```

- **Build numbers** are commit counts. Along `main` the difference between two is the number of commits between them, so "N commits behind" comes from the manifest alone, with no GitHub API calls and no rate limits.
- `update::Updater` holds the state and decides what to fetch. It is platform-independent and unit-tested; `platform::updater` does the HTTP (WinHTTP), hashing (BCrypt) and file swap.
- Builds without `GROUPLY_BUILD` (anything not built by CI from `main`) are development builds and never check.
- **Trust** rests on HTTPS and on control of the GitHub repository. The manifest's hash catches corrupt downloads, not a compromised release. Code signing would be the next step.

## Roadmap

1. **Run and harden on a WoA device**: z-order edge cases, Snap layouts, virtual desktops, multi-monitor DPI moves, UWP frames.
2. **Settings**: persist `Settings` (TOML under `%APPDATA%\Grouply`), run at sign-in and live reload are done; a small settings window is still missing.
3. **Bar polish**: per-pixel alpha (Direct2D into `UpdateLayeredWindow`) instead of a colour key; tab previews via DWM thumbnails; system accent colours. Tucked and hidden bars on maximized windows, revealed by touching the top edge, are done (polled).
4. **Rules**: auto-group by process, window class or title; saved groups.
5. **Elevated windows**: an optional signed build with `uiAccess=true`, installed under Program Files.
6. **Optional in-process features** (title-bar integration, replacing Explorer/Notepad tabs, Ctrl+T): these need hooks inside target processes. An ARM64X DLL loads into both ARM64 and x64-emulated processes, so it may let one file replace Groupy's separate ARM64 and x64 hook builds. Whether `SetWindowsHookEx` injects it into emulated processes still needs testing. A separate x86 build is required for 32-bit apps. This should stay opt-in, because the out-of-process core already covers the main feature.
