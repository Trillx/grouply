//! Win32 front end: one UI thread owning a hidden controller window, the hooks and the tab bars.

mod app;
mod autostart;
mod hooks;
mod tabbar;
mod tray;
mod updater;
mod win;

use std::cell::{Cell, RefCell};
use std::mem::size_of;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey, VK_OEM_3,
};
use windows::Win32::UI::Shell::NIN_BALLOONUSERCLICK;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use app::App;
use tabbar::WM_APP_TAB;
use tray::{MenuInfo, Tray, TrayChoice};
use updater::{Outcome, WM_APP_UPDATE};

use crate::command::TabCommand;
use crate::handoff::Launch;
use crate::settings::{self, Settings};
use crate::update::{self, BuildInfo, Job, Updater};

const CONTROLLER_CLASS: PCWSTR = w!("GrouplyController");
const WM_APP_TRAY: u32 = WM_APP + 3;
const HOTKEY_NEXT: i32 = 1;
const HOTKEY_PREV: i32 = 2;
const TIMER_UPDATE: usize = 2;
const TIMER_HOVER: usize = 3;
/// The first check waits a little, so the network is up when Grouply starts at sign-in.
const FIRST_CHECK_MS: u32 = 30_000;
const CHECK_INTERVAL_MS: u32 = 6 * 60 * 60 * 1000;
/// How often auto-hidden bars follow the pointer. Only runs when the setting is on.
const HOVER_POLL_MS: u32 = 100;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static UPDATER: RefCell<Updater> = RefCell::new(Updater::new(BuildInfo::current(), update::ARCH));
}

/// Runs `f` against the app unless it is already in use further up the stack.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Holds the per-session instance mutex, or returns None if Grouply is already running.
/// After an update, waits for the previous instance to exit instead.
fn single_instance(after_update: bool) -> windows::core::Result<Option<InstanceLock>> {
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\Grouply.Instance"))? };
    let lock = InstanceLock(handle);
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return Ok(Some(lock));
    }
    if after_update {
        // The old instance never releases the mutex; it is abandoned when that process exits.
        let wait = unsafe { WaitForSingleObject(handle, 15_000) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Ok(Some(lock));
        }
    }
    Ok(None)
}

pub fn run() -> windows::core::Result<()> {
    // Physical pixels everywhere, so positions match other apps' windows on every monitor.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let args = std::env::args_os()
        .skip(1)
        .filter_map(|a| a.into_string().ok());
    let launch = Launch::parse(args);
    let Some(_lock) = single_instance(launch.updated)? else {
        return Ok(());
    };
    updater::remove_leftovers();

    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(controller_proc),
        hInstance: instance,
        lpszClassName: CONTROLLER_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    tabbar::register_class(instance)?;

    // A real (hidden) top-level window rather than a message-only one: it owns popup menus and
    // receives broadcasts such as theme changes and TaskbarCreated.
    let controller = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CONTROLLER_CLASS,
            w!("Grouply"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?
    };
    TASKBAR_CREATED.set(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });

    let loaded = load_settings();
    let settings = loaded.settings.clone();
    register_hotkeys(controller, settings.cycle_hotkeys);
    configure_hover_timer(controller, settings.hide_when_maximized);
    APP.with_borrow_mut(|app| *app = Some(App::new(controller, instance, settings)));
    with_app(|app| app.adopt(&launch.groups));
    let tooltip = UPDATER.with_borrow(Updater::tooltip);
    TRAY.with_borrow_mut(|tray| {
        *tray = Some(Tray::add(controller, instance, WM_APP_TRAY, &tooltip))
    });
    notify_warnings(&loaded.warnings);
    if UPDATER.with_borrow(Updater::can_check) {
        let _ = unsafe { SetTimer(Some(controller), TIMER_UPDATE, FIRST_CHECK_MS, None) };
    }
    let hooks = hooks::Hooks::install(controller)?;

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    drop(hooks);
    if let Some(mut app) = APP.with_borrow_mut(Option::take) {
        app.shutdown();
    }
    TRAY.with_borrow_mut(Option::take);
    Ok(())
}

pub fn report_fatal(error: &windows::core::Error) {
    let text = HSTRING::from(format!("Grouply could not start.\n\n{error}"));
    unsafe { MessageBoxW(None, &text, w!("Grouply"), MB_ICONERROR | MB_OK) };
}

fn load_settings() -> settings::Loaded {
    match settings::config_path() {
        Some(path) => settings::load_or_create(&path),
        None => settings::Loaded::default(),
    }
}

fn notify(text: &str) {
    TRAY.with_borrow(|tray| {
        if let Some(tray) = tray {
            tray.notify("Grouply settings", text);
        }
    });
}

fn notify_warnings(warnings: &[String]) {
    if !warnings.is_empty() {
        notify(&warnings.join("\n"));
    }
}

/// Registers or releases the tab-cycling hotkeys, e.g. after a settings reload.
fn register_hotkeys(controller: HWND, enabled: bool) {
    unsafe {
        let _ = UnregisterHotKey(Some(controller), HOTKEY_NEXT);
        let _ = UnregisterHotKey(Some(controller), HOTKEY_PREV);
        if enabled {
            // Win+` and Win+Shift+` (the key below Esc on US layouts). Another app may own them.
            let _ = RegisterHotKey(
                Some(controller),
                HOTKEY_NEXT,
                MOD_WIN | MOD_NOREPEAT,
                VK_OEM_3.0 as u32,
            );
            let _ = RegisterHotKey(
                Some(controller),
                HOTKEY_PREV,
                MOD_WIN | MOD_SHIFT | MOD_NOREPEAT,
                VK_OEM_3.0 as u32,
            );
        }
    }
}

fn configure_hover_timer(controller: HWND, on: bool) {
    unsafe {
        if on {
            let _ = SetTimer(Some(controller), TIMER_HOVER, HOVER_POLL_MS, None);
        } else {
            let _ = KillTimer(Some(controller), TIMER_HOVER);
        }
    }
}

fn open_settings() {
    let Some(path) = settings::config_path() else {
        return;
    };
    if !path.exists() {
        let _ = Settings::default().save(&path);
    }
    if let Err(error) = std::process::Command::new("notepad.exe").arg(&path).spawn() {
        notify(&format!("couldn't open {}: {error}", path.display()));
    }
}

fn reload_settings(controller: HWND) {
    // Unlike startup, a missing file is reported rather than recreated, so a deleted file is
    // visible in the notification.
    let loaded = match settings::config_path() {
        Some(path) => settings::load(&path),
        None => settings::Loaded::default(),
    };
    register_hotkeys(controller, loaded.settings.cycle_hotkeys);
    configure_hover_timer(controller, loaded.settings.hide_when_maximized);
    with_app(|app| app.apply_settings(loaded.settings));
    notify_warnings(&loaded.warnings);
}

fn toggle_autostart() {
    let enable = !autostart::is_enabled();
    if let Err(error) = autostart::set_enabled(enable) {
        notify(&error);
    }
}

/// Starts the job `begin` hands out, if any, and shows the new state in the tray.
fn start_update(controller: HWND, begin: fn(&mut Updater) -> Option<Job>) {
    if let Some(job) = UPDATER.with_borrow_mut(begin) {
        updater::start(controller, job);
    }
    refresh_tray();
}

fn refresh_tray() {
    let tooltip = UPDATER.with_borrow(Updater::tooltip);
    TRAY.with_borrow_mut(|tray| {
        if let Some(tray) = tray.as_mut() {
            tray.set_tooltip(&tooltip);
        }
    });
}

fn menu_info() -> MenuInfo {
    UPDATER.with_borrow(|u| MenuInfo {
        version: format!("Grouply {}", u.build().label()),
        status: u.status().describe(),
        can_update: u.can_install(),
        can_check: u.can_check(),
        autostart: autostart::is_enabled(),
        settings_path: settings::config_path(),
    })
}

fn on_update_outcome(controller: HWND, outcome: Outcome) {
    match outcome {
        Outcome::Checked(result) => {
            let announce = UPDATER.with_borrow_mut(|u| u.checked(result));
            refresh_tray();
            if let Some(commits) = announce {
                let text = format!(
                    "{}. Click here to update and restart.",
                    update::behind_text(commits)
                );
                TRAY.with_borrow(|tray| {
                    if let Some(tray) = tray {
                        tray.notify("Grouply update available", &text);
                    }
                });
            }
        }
        Outcome::Downloaded(result) => {
            UPDATER.with_borrow_mut(Updater::download_finished);
            refresh_tray();
            let installed = result.and_then(|staged| {
                let groups = with_app(|app| app.saved_groups()).unwrap_or_default();
                updater::install(&staged, &groups)
            });
            match installed {
                // The new instance is waiting for this one to exit.
                Ok(()) => {
                    let _ = unsafe { DestroyWindow(controller) };
                }
                Err(error) => {
                    let text = HSTRING::from(format!("Grouply couldn't update.\n\n{error}"));
                    unsafe { MessageBoxW(None, &text, w!("Grouply"), MB_ICONWARNING | MB_OK) };
                }
            }
        }
    }
}

unsafe extern "system" fn controller_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        hooks::WM_APP_EVENTS => {
            let handled = with_app(|app| {
                for event in hooks::take() {
                    app.on_event(event);
                }
            });
            if handled.is_none() {
                hooks::repost();
            }
        }
        WM_APP_TAB => {
            if let Some(cmd) = TabCommand::decode(lparam.0 as u64) {
                with_app(|app| app.on_tab_command(wparam.0 as u32, cmd));
            }
        }
        WM_TIMER if wparam.0 == app::TIMER_DRAG => {
            with_app(App::on_drag_tick);
        }
        WM_TIMER if wparam.0 == TIMER_HOVER => {
            with_app(App::on_hover_tick);
        }
        WM_TIMER if wparam.0 == TIMER_UPDATE => {
            unsafe { SetTimer(Some(hwnd), TIMER_UPDATE, CHECK_INTERVAL_MS, None) };
            start_update(hwnd, Updater::begin_check);
        }
        WM_APP_UPDATE => {
            // SAFETY: posted by `updater::start`; each message is handled once.
            let outcome = unsafe { updater::take(lparam) };
            on_update_outcome(hwnd, outcome);
        }
        WM_HOTKEY => {
            let forward = wparam.0 as i32 == HOTKEY_NEXT;
            with_app(|app| app.cycle(forward));
        }
        WM_APP_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == NIN_BALLOONUSERCLICK {
                start_update(hwnd, Updater::begin_install);
            } else if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                // The menu runs a modal loop; the app must not be borrowed across it.
                match tray::show_menu(hwnd, &menu_info()) {
                    Some(TrayChoice::Update) => start_update(hwnd, Updater::begin_install),
                    Some(TrayChoice::CheckForUpdates) => start_update(hwnd, Updater::begin_check),
                    Some(TrayChoice::UngroupAll) => {
                        with_app(App::ungroup_all);
                    }
                    Some(TrayChoice::OpenSettings) => open_settings(),
                    Some(TrayChoice::ReloadSettings) => reload_settings(hwnd),
                    Some(TrayChoice::ToggleAutostart) => toggle_autostart(),
                    Some(TrayChoice::Exit) => {
                        let _ = unsafe { DestroyWindow(hwnd) };
                    }
                    None => {}
                }
            }
        }
        WM_SETTINGCHANGE => {
            with_app(App::refresh_theme);
        }
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        _ if msg != 0 && msg == TASKBAR_CREATED.get() => {
            TRAY.with_borrow(|tray| tray.as_ref().map(Tray::show));
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}
