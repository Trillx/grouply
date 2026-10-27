//! Thin wrappers over the Win32 calls Grouply makes against other applications' windows.
//!
//! Everything here works out of process: no code is injected, so the same calls manage native
//! ARM64, emulated x64/x86 and ARM64EC applications alike.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, w};

use crate::layout::Rect;

/// A window handle that can be hashed and stored in collections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Win(pub isize);

impl Win {
    pub fn hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

impl From<HWND> for Win {
    fn from(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
}

/// Shell and system surfaces that look like ordinary windows but must never be grouped.
const EXCLUDED_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "ForegroundStaging",
    "MultitaskingViewFrame",
];

fn to_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

pub fn exists(w: Win) -> bool {
    unsafe { IsWindow(Some(w.hwnd())).as_bool() }
}

pub fn is_visible(w: Win) -> bool {
    unsafe { IsWindowVisible(w.hwnd()).as_bool() }
}

pub fn is_minimized(w: Win) -> bool {
    unsafe { IsIconic(w.hwnd()).as_bool() }
}

pub fn is_maximized(w: Win) -> bool {
    unsafe { IsZoomed(w.hwnd()).as_bool() }
}

fn ex_style(w: Win) -> u32 {
    unsafe { GetWindowLongPtrW(w.hwnd(), GWL_EXSTYLE) as u32 }
}

pub fn is_topmost(w: Win) -> bool {
    ex_style(w) & WS_EX_TOPMOST.0 != 0
}

pub fn is_cloaked(w: Win) -> bool {
    let mut cloaked = 0u32;
    let got = unsafe {
        DwmGetWindowAttribute(
            w.hwnd(),
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            size_of::<u32>() as u32,
        )
    };
    got.is_ok() && cloaked != 0
}

pub fn process_id(w: Win) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(w.hwnd(), Some(&mut pid)) };
    pid
}

pub fn class_name(w: Win) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(w.hwnd(), &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..len])
}

/// The window's title as last set, read without sending it a message (safe for hung windows).
pub fn title(w: Win) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { InternalGetWindowText(w.hwnd(), &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..len])
}

pub fn small_icon(w: Win) -> HICON {
    let hwnd = w.hwnd();
    for kind in [ICON_SMALL2, ICON_SMALL, ICON_BIG] {
        let mut icon = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                100,
                Some(&mut icon),
            )
        };
        if sent.0 != 0 && icon != 0 {
            return HICON(icon as *mut _);
        }
    }
    for index in [GCLP_HICONSM, GCLP_HICON] {
        let icon = unsafe { GetClassLongPtrW(hwnd, index) };
        if icon != 0 {
            return HICON(icon as *mut _);
        }
    }
    unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default()
}

/// Visible frame bounds in physical pixels, excluding the invisible resize borders.
pub fn frame(w: Win) -> Option<Rect> {
    let mut r = RECT::default();
    let got = unsafe {
        DwmGetWindowAttribute(
            w.hwnd(),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut _,
            size_of::<RECT>() as u32,
        )
    };
    match got {
        Ok(()) if !to_rect(r).is_empty() => Some(to_rect(r)),
        _ => window_rect(w),
    }
}

fn window_rect(w: Win) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(w.hwnd(), &mut r) }.ok()?;
    Some(to_rect(r))
}

pub fn work_area(w: Win) -> Rect {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let monitor = MonitorFromWindow(w.hwnd(), MONITOR_DEFAULTTONEAREST);
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    to_rect(info.rcWork)
}

/// Effective DPI of the monitor the window is on. Unlike `GetDpiForWindow`, this is right for
/// DPI-unaware windows too, which matters because Grouply works in physical pixels.
pub fn dpi(w: Win) -> u32 {
    let (mut x, mut y) = (96u32, 96u32);
    unsafe {
        let monitor = MonitorFromWindow(w.hwnd(), MONITOR_DEFAULTTONEAREST);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_err() {
            return 96;
        }
    }
    x
}

/// The window directly above `w` in the z-order.
pub fn above(w: Win) -> Option<Win> {
    unsafe { GetWindow(w.hwnd(), GW_HWNDPREV) }
        .ok()
        .map(Win::from)
}

pub fn foreground() -> Option<Win> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then(|| Win::from(hwnd))
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Top-level windows, topmost first.
pub fn top_level_windows() -> Vec<Win> {
    unsafe extern "system" fn collect(hwnd: HWND, out: LPARAM) -> BOOL {
        let out = unsafe { &mut *(out.0 as *mut Vec<Win>) };
        out.push(Win::from(hwnd));
        true.into()
    }
    let mut out: Vec<Win> = Vec::new();
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut out as *mut Vec<Win> as isize)) };
    out
}

/// Whether `w` is something under the cursor that hides what is behind it.
pub fn is_opaque_surface(w: Win) -> bool {
    is_visible(w) && !is_cloaked(w) && ex_style(w) & WS_EX_TRANSPARENT.0 == 0
}

/// Whether `w` is an ordinary application window Grouply may put in a group.
pub fn is_groupable(w: Win) -> bool {
    let hwnd = w.hwnd();
    if !exists(w) || !is_visible(w) || is_cloaked(w) {
        return false;
    }
    unsafe {
        if GetAncestor(hwnd, GA_ROOT) != hwnd || GetWindow(hwnd, GW_OWNER).is_ok() {
            return false;
        }
    }
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    if style & WS_CHILD.0 != 0 || style & (WS_CAPTION.0 | WS_THICKFRAME.0) == 0 {
        return false;
    }
    if ex_style(w) & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 {
        return false;
    }
    let pid = process_id(w);
    if pid == unsafe { GetCurrentProcessId() } || unsafe { IsHungAppWindow(hwnd) }.as_bool() {
        return false;
    }
    !EXCLUDED_CLASSES.contains(&class_name(w).as_str()) && can_manage(pid)
}

/// Windows of processes above our integrity level (elevated apps) refuse our moves; leave them alone.
fn can_manage(pid: u32) -> bool {
    thread_local! {
        static CACHE: RefCell<HashMap<u32, bool>> = RefCell::new(HashMap::new());
    }
    static OURS: OnceLock<u32> = OnceLock::new();
    if let Some(known) = CACHE.with_borrow(|c| c.get(&pid).copied()) {
        return known;
    }
    let ours = *OURS.get_or_init(|| integrity_level(unsafe { GetCurrentProcess() }).unwrap_or(0));
    let theirs = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .ok()
        .and_then(|process| {
            let level = integrity_level(process);
            let _ = unsafe { CloseHandle(process) };
            level
        });
    let allowed = theirs.is_some_and(|level| level <= ours);
    CACHE.with_borrow_mut(|c| c.insert(pid, allowed));
    allowed
}

fn integrity_level(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    // u64 storage keeps the TOKEN_MANDATORY_LABEL read aligned.
    let mut buf = [0u64; 16];
    let mut len = 0u32;
    let got = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut _),
            size_of_val(&buf) as u32,
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(token) };
    got.ok()?;
    unsafe {
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        Some(*GetSidSubAuthority(
            label.Label.Sid,
            u32::from(count.checked_sub(1)?),
        ))
    }
}

/// Moves and sizes `w` so its visible frame matches `target`, restoring it first when it is
/// minimized or maximized. Posted asynchronously so a hung application cannot stall Grouply.
pub fn set_frame(w: Win, target: Rect) {
    let hwnd = w.hwnd();
    let minimized = is_minimized(w);
    if minimized || is_maximized(w) {
        unsafe {
            let _ = ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
        }
    }
    // Windows differ in invisible border width; keep w's own. A minimized window reports no
    // usable frame, so it gets none until the next sync.
    let (l, t, r, b) = match (minimized, window_rect(w), frame(w)) {
        (false, Some(outer), Some(visible)) => (
            visible.left - outer.left,
            visible.top - outer.top,
            outer.right - visible.right,
            outer.bottom - visible.bottom,
        ),
        _ => (0, 0, 0, 0),
    };
    let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            target.left - l,
            target.top - t,
            target.width() + l + r,
            target.height() + t + b,
            flags,
        )
    };
}

/// Puts `w` directly below `above` in the z-order without activating it.
pub fn place_below(w: Win, above: Win) {
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
    let _ = unsafe { SetWindowPos(w.hwnd(), Some(above.hwnd()), 0, 0, 0, 0, flags) };
}

/// Brings `w` forward and gives it focus. Only succeeds while Grouply owns the latest input
/// (a click on a tab or a hotkey), which is exactly when it is called.
pub fn activate(w: Win) {
    unsafe {
        if is_minimized(w) {
            let _ = ShowWindowAsync(w.hwnd(), SW_RESTORE);
        }
        let _ = SetForegroundWindow(w.hwnd());
    }
}

pub fn maximize(w: Win) {
    unsafe {
        let _ = ShowWindowAsync(w.hwnd(), SW_MAXIMIZE);
    }
}

pub fn minimize_quietly(w: Win) {
    unsafe {
        let _ = ShowWindowAsync(w.hwnd(), SW_SHOWMINNOACTIVE);
    }
}

pub fn restore_quietly(w: Win) {
    unsafe {
        let _ = ShowWindowAsync(w.hwnd(), SW_SHOWNOACTIVATE);
    }
}

/// Asks the window to close as if its close button were clicked.
pub fn close(w: Win) {
    let _ = unsafe {
        PostMessageW(
            Some(w.hwnd()),
            WM_SYSCOMMAND,
            WPARAM(SC_CLOSE as usize),
            LPARAM(0),
        )
    };
}

/// Whether apps should use the light theme, per the user's personalisation settings.
pub fn light_theme() -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    status.is_err() || value != 0
}
