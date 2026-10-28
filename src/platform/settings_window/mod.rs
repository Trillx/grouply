//! The settings window: a live preview of the tab bar above presets, dials, toggles and the
//! maximized-window mode, laid out by [`crate::settings_ui`].
//!
//! Changes apply as they are made. The window edits its own copy of the settings and posts
//! [`WM_APP_SETTINGS`] to the controller, which applies the copy to the running app and saves the
//! file shortly after the last change. Everything is owner-drawn: GDI+ for the anti-aliased shapes,
//! GDI for text and for the preview's tabs, which reuse the tab bar's own painting.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::{SetScrollInfo, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VIRTUAL_KEY, VK_BACK, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    SHGSI_ICON, SHGSI_SMALLICON, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_APPLICATION,
    SIID_DOCNOASSOC, SIID_FOLDER,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use super::tabbar::{self, TabView};
use super::{autostart, tray, win};
use crate::layout::Rect;
use crate::settings::{Field, Settings};
use crate::settings_ui::{self, Control, Layout, PRESETS, Toggle};

mod canvas;
mod paint;

use paint::paint;

/// Posted to the controller; `wparam` is [`CHANGED`], [`CLOSED`] or [`OPEN_FILE`].
pub const WM_APP_SETTINGS: u32 = WM_APP + 5;
/// The window's settings differ from the app's: apply them and save soon.
pub const CHANGED: usize = 0;
/// The window closed: save now if a save is pending.
pub const CLOSED: usize = 1;
/// The user asked for the file: save, then open it.
pub const OPEN_FILE: usize = 2;

const CLASS: PCWSTR = w!("GrouplySettings");
const STYLE: WINDOW_STYLE = WINDOW_STYLE(
    WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_THICKFRAME.0,
);

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(HWND(null_mut())) };
}

#[derive(Clone, Copy)]
struct Theme {
    light: bool,
    bg: u32,
    card: u32,
    hover: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    track: u32,
    /// Fill of a selected card or a toggle that is on.
    selected: u32,
    desk: (u32, u32),
    /// The sample window's client area and caption.
    window: u32,
    caption: u32,
}

const LIGHT: Theme = Theme {
    light: true,
    bg: 0xF3F3F3,
    card: 0xFFFFFF,
    hover: 0xF6F6F6,
    border: 0xE0E0E0,
    text: 0x1A1A1A,
    muted: 0x616161,
    accent: 0x059669,
    track: 0xD4D4D4,
    selected: 0xE6F4EE,
    desk: (0xC9D8EE, 0x9DB6DE),
    window: 0xFFFFFF,
    caption: 0xF3F3F3,
};

const DARK: Theme = Theme {
    light: false,
    bg: 0x202020,
    card: 0x2B2B2B,
    hover: 0x323232,
    border: 0x3D3D3D,
    text: 0xFFFFFF,
    muted: 0xA8A8A8,
    accent: 0x10B981,
    track: 0x4A4A4A,
    selected: 0x1C3B30,
    desk: (0x22324D, 0x0F1626),
    window: 0x191919,
    caption: 0x202020,
};

fn theme(light: bool) -> Theme {
    if light { LIGHT } else { DARK }
}

/// `0xRRGGBB` as a GDI colour.
fn cref(c: u32) -> COLORREF {
    COLORREF(((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF))
}

/// `0xRRGGBB` as an opaque GDI+ colour.
fn argb(c: u32) -> u32 {
    0xFF00_0000 | c
}

struct Fonts {
    body: HFONT,
    strong: HFONT,
    small: HFONT,
    heading: HFONT,
    value: HFONT,
    /// The tab bar's own font, for the preview.
    tab: HFONT,
}

impl Fonts {
    fn new(dpi: u32) -> Self {
        let font = |px: i32, weight: FONT_WEIGHT| unsafe {
            CreateFontW(
                -(px * dpi as i32 / 96),
                0,
                0,
                0,
                weight.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        };
        Self {
            body: font(14, FW_NORMAL),
            strong: font(14, FW_SEMIBOLD),
            small: font(12, FW_NORMAL),
            heading: font(15, FW_SEMIBOLD),
            value: font(15, FW_SEMIBOLD),
            tab: tabbar::make_font(dpi),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [
            self.body,
            self.strong,
            self.small,
            self.heading,
            self.value,
            self.tab,
        ] {
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(font.into());
                }
            }
        }
    }
}

/// The tabs shown in the preview, with stock shell icons.
struct Sample(Vec<TabView>);

impl Sample {
    fn new() -> Self {
        let tabs = [
            (SIID_DOCNOASSOC, "notes.txt – Notepad"),
            (SIID_FOLDER, "Downloads – File Explorer"),
            (SIID_APPLICATION, "Windows PowerShell"),
        ]
        .into_iter()
        .map(|(siid, title)| {
            let mut info = SHSTOCKICONINFO {
                cbSize: size_of::<SHSTOCKICONINFO>() as u32,
                ..Default::default()
            };
            let ok = unsafe { SHGetStockIconInfo(siid, SHGSI_ICON | SHGSI_SMALLICON, &mut info) };
            TabView {
                title: title.to_owned(),
                icon: if ok.is_ok() {
                    info.hIcon
                } else {
                    HICON::default()
                },
            }
        })
        .collect();
        Self(tabs)
    }
}

impl Drop for Sample {
    fn drop(&mut self) {
        for tab in &self.0 {
            if !tab.icon.is_invalid() {
                unsafe {
                    let _ = DestroyIcon(tab.icon);
                }
            }
        }
    }
}

/// A dial or slider being dragged.
struct Drag {
    field: Field,
    dial: bool,
    /// The value and pointer height when the drag began; dials move relative to them.
    start: i32,
    y: i32,
}

struct State {
    controller: HWND,
    settings: Settings,
    autostart: bool,
    theme: Theme,
    dpi: u32,
    /// Client width and height.
    client: (i32, i32),
    /// Showing the sizes without a dial.
    more: bool,
    layout: Layout,
    scroll: i32,
    fonts: Fonts,
    sample: Sample,
    hover: Option<Control>,
    focus: Option<Control>,
    /// Draw the focus ring: the keyboard was used last.
    cues: bool,
    /// A button-like control held down, activated on release over it.
    pressed: Option<Control>,
    drag: Option<Drag>,
    /// Digits typed into the focused dial or slider, applied with Enter.
    typing: Option<String>,
    tracking: bool,
}

impl State {
    fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }

    fn relayout(&mut self) {
        self.layout = Layout::new(self.client.0, self.dpi, self.more);
        self.scroll = self.scroll.clamp(0, self.layout.max_scroll(self.client.1));
    }

    /// Sets a size; true if any setting changed (tab limits can move each other).
    fn set(&mut self, field: Field, value: i32) -> bool {
        let before = self.settings.clone();
        field.set(&mut self.settings, value);
        self.settings != before
    }

    fn toggle(&self, toggle: Toggle) -> bool {
        match toggle {
            Toggle::MiddleClick => self.settings.middle_click_closes,
            Toggle::CycleHotkeys => self.settings.cycle_hotkeys,
            Toggle::Autostart => self.autostart,
        }
    }

    /// The slider value under client x.
    fn slider_value(&self, field: Field, x: i32) -> i32 {
        let Some(r) = self.layout.rect(Control::Slider(field)) else {
            return field.get(&self.settings);
        };
        let (left, right, _) = track(r, self.dpi);
        settings_ui::value_at(field, (x - left) as f32 / (right - left).max(1) as f32)
    }
}

/// A slider's track: left end, right end and height, in page coordinates.
fn track(r: Rect, dpi: u32) -> (i32, i32, i32) {
    let s = |v: i32| v * dpi as i32 / 96;
    (r.left + s(9), r.right - s(9), r.top + s(32))
}

/// Things to do once the state is no longer borrowed: several of these re-enter the window
/// procedure synchronously.
#[derive(Default)]
struct Effects {
    controller: HWND,
    repaint: bool,
    scrollbar: bool,
    capture: Option<bool>,
    track_leave: bool,
    /// The window rectangle Windows suggested for a new DPI.
    resize: Option<RECT>,
    dark_title: Option<bool>,
    post: Option<usize>,
    error: Option<String>,
    close: bool,
}

pub fn register_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let icon = |cx, cy| unsafe {
        LoadImageW(
            Some(instance),
            tray::APP_ICON,
            IMAGE_ICON,
            GetSystemMetrics(cx),
            GetSystemMetrics(cy),
            LR_DEFAULTCOLOR,
        )
        .map(|image| HICON(image.0))
        .unwrap_or_default()
    };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hIcon: icon(SM_CXICON, SM_CYICON),
        hIconSm: icon(SM_CXSMICON, SM_CYSMICON),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Opens the window on the monitor under the pointer, or brings it forward if it is open.
pub fn open(instance: HINSTANCE, controller: HWND, settings: Settings) {
    let existing = WINDOW.get();
    if !existing.is_invalid() {
        unsafe {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
        }
        return;
    }
    let (x, y) = win::cursor_pos();
    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
    let work = info.rcWork;
    let (mut dpi, mut dpi_y) = (96, 96);
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y) };

    let s = |v: i32| v * dpi as i32 / 96;
    let width = s(settings_ui::WIDTH);
    let layout = Layout::new(width, dpi, false);
    let height = layout
        .height
        .min(work.bottom - work.top - s(96))
        .max(s(320));
    let (outer_w, outer_h) = outer_size(dpi, width, height);
    let left = work.left + (work.right - work.left - outer_w) / 2;
    let top = work.top + (work.bottom - work.top - outer_h) / 2;

    let light = win::light_theme();
    let state = Box::new(RefCell::new(State {
        controller,
        settings,
        autostart: autostart::is_enabled(),
        theme: theme(light),
        dpi,
        client: (width, height),
        more: false,
        layout,
        scroll: 0,
        fonts: Fonts::new(dpi),
        sample: Sample::new(),
        hover: None,
        focus: None,
        cues: false,
        pressed: None,
        drag: None,
        typing: None,
        tracking: false,
    }));
    // The window owns the state from WM_NCCREATE and frees it in WM_NCDESTROY. If creation
    // fails before that, the state leaks; that only happens when the class is missing.
    let created = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Grouply settings"),
            STYLE | WS_VSCROLL,
            left,
            top,
            outer_w,
            outer_h,
            None,
            None,
            Some(instance),
            Some(Box::into_raw(state) as *const c_void),
        )
    };
    if let Ok(hwnd) = created {
        WINDOW.set(hwnd);
        set_dark_title(hwnd, !light);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// The settings as the window has them, for the controller to apply.
pub fn settings() -> Option<Settings> {
    with_state(|st| st.settings.clone())
}

/// Shows settings that changed elsewhere, e.g. reloaded from the file.
pub fn reload(settings: &Settings) {
    with_state(|st| {
        st.settings = settings.clone();
        st.typing = None;
    });
    repaint();
}

/// Shows a Start with Windows change made from the tray menu.
pub fn refresh_autostart() {
    with_state(|st| st.autostart = autostart::is_enabled());
    repaint();
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    let hwnd = WINDOW.get();
    if hwnd.is_invalid() {
        return None;
    }
    with_cell(hwnd, |cell| {
        cell.try_borrow_mut().ok().map(|mut st| f(&mut st))
    })
    .flatten()
}

fn repaint() {
    let hwnd = WINDOW.get();
    if !hwnd.is_invalid() {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Window size for a client area, with room for the vertical scroll bar.
fn outer_size(dpi: u32, width: i32, height: i32) -> (i32, i32) {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let _ = unsafe { AdjustWindowRectExForDpi(&mut rc, STYLE, false, WINDOW_EX_STYLE(0), dpi) };
    let scrollbar = unsafe { GetSystemMetricsForDpi(SM_CXVSCROLL, dpi) };
    (rc.right - rc.left + scrollbar, rc.bottom - rc.top)
}

fn set_dark_title(hwnd: HWND, dark: bool) {
    let value = BOOL::from(dark);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const BOOL as *const c_void,
            size_of::<BOOL>() as u32,
        )
    };
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            WINDOW.set(HWND(null_mut()));
            // Nothing holds the state now: handlers let go of it before DefWindowProc or
            // `apply` can destroy the window.
            drop(Box::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let mut fx = Effects::default();
        let handled = {
            let cell = &*ptr;
            if msg == WM_PAINT {
                paint(hwnd, cell);
                Some(LRESULT(0))
            } else {
                match cell.try_borrow_mut() {
                    Ok(mut st) => on_message(hwnd, &mut st, msg, wparam, lparam, &mut fx),
                    Err(_) => None,
                }
            }
        };
        // DefWindowProc may destroy the window (WM_CLOSE, SC_CLOSE), freeing the state.
        let result = handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam));
        apply(hwnd, fx);
        result
    }
}

/// Runs `f` on the window's state, if it still has one.
fn with_cell<R>(hwnd: HWND, f: impl FnOnce(&RefCell<State>) -> R) -> Option<R> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<State>;
    // SAFETY: the pointer is cleared in WM_NCDESTROY before the state is freed, and the
    // reference doesn't outlive `f`.
    (!ptr.is_null()).then(|| f(unsafe { &*ptr }))
}

/// Handles a message with the state borrowed. None passes it on to DefWindowProc, which runs
/// after the borrow ends.
unsafe fn on_message(
    hwnd: HWND,
    st: &mut State,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    fx: &mut Effects,
) -> Option<LRESULT> {
    fx.controller = st.controller;
    unsafe {
        match msg {
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_DESTROY => fx.post = Some(CLOSED),
            WM_SIZE => {
                st.client = point(lparam);
                st.relayout();
                fx.scrollbar = true;
                fx.repaint = true;
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                let (w, h) = outer_size(st.dpi, st.s(settings_ui::WIDTH), st.s(320));
                info.ptMinTrackSize = POINT { x: w, y: h };
                info.ptMaxTrackSize.x = w;
            }
            WM_DPICHANGED => {
                st.dpi = ((wparam.0 >> 16) & 0xFFFF) as u32;
                st.fonts = Fonts::new(st.dpi);
                st.relayout();
                // The resize re-enters with WM_SIZE, which lays the page out again.
                fx.resize = Some(*(lparam.0 as *const RECT));
                fx.repaint = true;
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                let light = win::light_theme();
                if light != st.theme.light {
                    st.theme = theme(light);
                    fx.dark_title = Some(!light);
                    fx.repaint = true;
                }
                return None;
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let _ = ScreenToClient(hwnd, &mut p);
                let cursor = match st.drag.as_ref().map(|d| d.dial) {
                    Some(true) => IDC_SIZENS,
                    Some(false) => IDC_HAND,
                    None => match st.layout.hit(p.x, p.y, st.scroll) {
                        Some(Control::Dial(_)) => IDC_SIZENS,
                        Some(_) => IDC_HAND,
                        None => IDC_ARROW,
                    },
                };
                if let Ok(c) = LoadCursorW(None, cursor) {
                    SetCursor(Some(c));
                }
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                let (x, y) = point(lparam);
                on_move(st, x, y, fx);
            }
            WM_MOUSELEAVE => {
                st.tracking = false;
                st.hover = None;
                fx.repaint = true;
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = point(lparam);
                on_down(st, x, y, msg == WM_LBUTTONDBLCLK, fx);
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lparam);
                on_up(st, x, y, fx);
            }
            WM_CAPTURECHANGED => {
                st.drag = None;
                st.pressed = None;
                fx.repaint = true;
            }
            WM_MOUSEWHEEL => {
                let (x, y) = point(lparam);
                let mut p = POINT { x, y };
                let _ = ScreenToClient(hwnd, &mut p);
                let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                on_wheel(st, p.x, p.y, delta, fx);
            }
            WM_VSCROLL => on_vscroll(hwnd, st, (wparam.0 & 0xFFFF) as i32, fx),
            WM_KEYDOWN => {
                if !on_key(st, VIRTUAL_KEY(wparam.0 as u16), fx) {
                    return None;
                }
            }
            WM_CHAR => {
                let typed = char::from_u32(wparam.0 as u32);
                if let (Some(c @ '0'..='9'), Some(Control::Dial(_) | Control::Slider(_))) =
                    (typed, st.focus)
                {
                    let digits = st.typing.get_or_insert_with(String::new);
                    if digits.len() < 4 {
                        digits.push(c);
                    }
                    fx.repaint = true;
                }
            }
            WM_KILLFOCUS => {
                st.typing = None;
                fx.repaint = true;
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

unsafe fn apply(hwnd: HWND, fx: Effects) {
    unsafe {
        match fx.capture {
            Some(true) => {
                SetCapture(hwnd);
            }
            Some(false) => {
                let _ = ReleaseCapture();
            }
            None => {}
        }
        if fx.track_leave {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
        }
        if fx.scrollbar {
            // Showing or hiding the scroll bar resizes the client area, which re-enters with
            // WM_SIZE, so the state can't be borrowed across this call.
            let info = with_cell(hwnd, |cell| {
                cell.try_borrow().ok().map(|st| SCROLLINFO {
                    cbSize: size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                    nMin: 0,
                    nMax: st.layout.height - 1,
                    nPage: st.client.1.max(0) as u32,
                    nPos: st.scroll,
                    nTrackPos: 0,
                })
            })
            .flatten();
            if let Some(info) = info {
                SetScrollInfo(hwnd, SB_VERT, &info, true);
            }
        }
        if let Some(r) = fx.resize {
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if let Some(dark) = fx.dark_title {
            set_dark_title(hwnd, dark);
        }
        if let Some(code) = fx.post {
            let _ = PostMessageW(
                Some(fx.controller),
                WM_APP_SETTINGS,
                WPARAM(code),
                LPARAM(0),
            );
        }
        if fx.repaint {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        if let Some(error) = fx.error {
            MessageBoxW(
                Some(hwnd),
                &HSTRING::from(error),
                w!("Grouply"),
                MB_ICONWARNING | MB_OK,
            );
        }
        if fx.close {
            let _ = DestroyWindow(hwnd);
        }
    }
}

fn changed(fx: &mut Effects) {
    fx.post = Some(CHANGED);
    fx.repaint = true;
}

fn on_move(st: &mut State, x: i32, y: i32, fx: &mut Effects) {
    if let Some(d) = &st.drag {
        let value = if d.dial {
            settings_ui::dial_drag(d.field, d.start, y - d.y, st.dpi)
        } else {
            st.slider_value(d.field, x)
        };
        if st.set(d.field, value) {
            changed(fx);
        }
        return;
    }
    let hit = st.layout.hit(x, y, st.scroll);
    if hit != st.hover {
        st.hover = hit;
        fx.repaint = true;
    }
    if !st.tracking {
        st.tracking = true;
        fx.track_leave = true;
    }
}

fn on_down(st: &mut State, x: i32, y: i32, double: bool, fx: &mut Effects) {
    st.typing = None;
    st.cues = false;
    fx.repaint = true;
    let Some(control) = st.layout.hit(x, y, st.scroll) else {
        return;
    };
    st.focus = Some(control);
    match control {
        Control::Dial(field) if double => {
            if st.set(field, field.get(&Settings::default())) {
                changed(fx);
            }
        }
        Control::Dial(field) => {
            st.drag = Some(Drag {
                field,
                dial: true,
                start: field.get(&st.settings),
                y,
            });
            fx.capture = Some(true);
        }
        Control::Slider(field) => {
            st.drag = Some(Drag {
                field,
                dial: false,
                start: field.get(&st.settings),
                y,
            });
            fx.capture = Some(true);
            if st.set(field, st.slider_value(field, x)) {
                changed(fx);
            }
        }
        _ => {
            st.pressed = Some(control);
            fx.capture = Some(true);
        }
    }
}

fn on_up(st: &mut State, x: i32, y: i32, fx: &mut Effects) {
    st.drag = None;
    let pressed = st.pressed.take();
    fx.capture = Some(false);
    fx.repaint = true;
    if let Some(control) = pressed
        && st.layout.hit(x, y, st.scroll) == Some(control)
    {
        activate(st, control, fx);
    }
}

fn activate(st: &mut State, control: Control, fx: &mut Effects) {
    fx.repaint = true;
    match control {
        Control::Preset(i) => {
            let metrics = PRESETS[i].apply(st.settings.metrics);
            if metrics != st.settings.metrics {
                st.settings.metrics = metrics;
                changed(fx);
            }
        }
        Control::Toggle(Toggle::MiddleClick) => {
            st.settings.middle_click_closes = !st.settings.middle_click_closes;
            changed(fx);
        }
        Control::Toggle(Toggle::CycleHotkeys) => {
            st.settings.cycle_hotkeys = !st.settings.cycle_hotkeys;
            changed(fx);
        }
        Control::Toggle(Toggle::Autostart) => match autostart::set_enabled(!st.autostart) {
            Ok(()) => st.autostart = !st.autostart,
            Err(error) => fx.error = Some(error),
        },
        Control::Mode(mode) => {
            if st.settings.maximized != mode {
                st.settings.maximized = mode;
                changed(fx);
            }
        }
        Control::More => {
            st.more = !st.more;
            st.relayout();
            if st.more
                && let Some(&last) = settings_ui::SLIDERS.last()
            {
                st.scroll = st
                    .layout
                    .scroll_to(Control::Slider(last), st.scroll, st.client.1);
            }
            fx.scrollbar = true;
        }
        Control::Reset => {
            let defaults = Settings::default();
            if st.settings != defaults {
                st.settings = defaults;
                changed(fx);
            }
        }
        Control::OpenFile => fx.post = Some(OPEN_FILE),
        Control::Dial(_) | Control::Slider(_) => {}
    }
}

fn set_scroll(st: &mut State, scroll: i32, fx: &mut Effects) {
    let scroll = scroll.clamp(0, st.layout.max_scroll(st.client.1));
    if scroll != st.scroll {
        st.scroll = scroll;
        fx.scrollbar = true;
        fx.repaint = true;
    }
}

fn on_wheel(st: &mut State, x: i32, y: i32, delta: i32, fx: &mut Effects) {
    // Turning the wheel over a dial turns the dial; anywhere else it scrolls the page.
    if let Some(Control::Dial(field)) = st.layout.hit(x, y, st.scroll) {
        let value = field.get(&st.settings) + settings_ui::step(field) * delta.signum();
        if st.set(field, value) {
            changed(fx);
        }
        return;
    }
    let scroll = st.scroll - delta * st.s(48) / 120;
    set_scroll(st, scroll, fx);
}

fn on_vscroll(hwnd: HWND, st: &mut State, code: i32, fx: &mut Effects) {
    let line = st.s(48);
    let page = (st.client.1 - st.layout.header).max(line);
    let scroll = match SCROLLBAR_COMMAND(code) {
        SB_LINEUP => st.scroll - line,
        SB_LINEDOWN => st.scroll + line,
        SB_PAGEUP => st.scroll - page,
        SB_PAGEDOWN => st.scroll + page,
        SB_TOP => 0,
        SB_BOTTOM => i32::MAX,
        SB_THUMBTRACK | SB_THUMBPOSITION => {
            let mut info = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_TRACKPOS,
                ..Default::default()
            };
            match unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info) } {
                Ok(()) => info.nTrackPos,
                Err(_) => st.scroll,
            }
        }
        _ => st.scroll,
    };
    set_scroll(st, scroll, fx);
}

fn move_focus(st: &mut State, backwards: bool, fx: &mut Effects) {
    st.focus = st.layout.next_focus(st.focus, backwards);
    st.typing = None;
    if let Some(control) = st.focus {
        let scroll = st.layout.scroll_to(control, st.scroll, st.client.1);
        set_scroll(st, scroll, fx);
    }
}

/// Returns false for keys the window doesn't use.
fn on_key(st: &mut State, key: VIRTUAL_KEY, fx: &mut Effects) -> bool {
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let cues = std::mem::replace(&mut st.cues, true);
    fx.repaint = true;
    let value_field = match st.focus {
        Some(Control::Dial(field) | Control::Slider(field)) => Some(field),
        _ => None,
    };
    match key {
        VK_TAB => move_focus(st, shift, fx),
        VK_ESCAPE => {
            if st.typing.take().is_none() {
                fx.close = true;
            }
        }
        VK_RETURN | VK_SPACE => {
            if let Some(digits) = st.typing.take() {
                if let (Some(field), Ok(value)) = (value_field, digits.parse::<i32>())
                    && st.set(field, value)
                {
                    changed(fx);
                }
            } else if let Some(control) = st.focus {
                activate(st, control, fx);
            }
        }
        VK_BACK => {
            if let Some(digits) = st.typing.as_mut() {
                digits.pop();
                if digits.is_empty() {
                    st.typing = None;
                }
            }
        }
        VK_UP | VK_RIGHT | VK_DOWN | VK_LEFT | VK_PRIOR | VK_NEXT | VK_HOME | VK_END => {
            if let Some(field) = value_field {
                st.typing = None;
                let step = settings_ui::step(field);
                let (min, max) = settings_ui::control_range(field);
                let current = field.get(&st.settings);
                let value = match key {
                    VK_HOME => min,
                    VK_END => max,
                    VK_PRIOR => current + 10 * step,
                    VK_NEXT => current - 10 * step,
                    VK_UP | VK_RIGHT => current + if shift { 10 * step } else { step },
                    _ => current - if shift { 10 * step } else { step },
                };
                if st.set(field, value) {
                    changed(fx);
                }
            } else {
                match key {
                    VK_HOME => set_scroll(st, 0, fx),
                    VK_END => set_scroll(st, i32::MAX, fx),
                    VK_PRIOR | VK_NEXT => {
                        let page = st.client.1 - st.layout.header;
                        let scroll = st.scroll + if key == VK_PRIOR { -page } else { page };
                        set_scroll(st, scroll, fx);
                    }
                    _ => move_focus(st, key == VK_UP || key == VK_LEFT, fx),
                }
            }
        }
        _ => {
            st.cues = cues;
            fx.repaint = false;
            return false;
        }
    }
    true
}
