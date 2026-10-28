//! The tab strip shown above a group's active window.
//!
//! The bar is a layered, colour-keyed popup: only the tabs are painted, the rest is transparent
//! and click-through. It never takes focus (`WS_EX_NOACTIVATE`), so clicking a tab leaves Grouply
//! holding the last input event, which is what lets it bring the chosen window forward. User
//! actions are posted to the controller as [`TabCommand`]s rather than handled here.

use std::cell::RefCell;
use std::mem::size_of;
use std::rc::Rc;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::win::{self, Win};
use crate::command::TabCommand;
use crate::layout::{self, BarMetrics, Hit, Rect, TabGeom};
use crate::model::GroupId;

/// Posted to the controller: `wparam` is the group id, `lparam` an encoded [`TabCommand`].
pub const WM_APP_TAB: u32 = WM_APP + 2;
const _: () = assert!(
    size_of::<isize>() == 8,
    "TabCommand packing needs a 64-bit LPARAM"
);

const CLASS: PCWSTR = w!("GrouplyTabBar");
/// Colour key: pixels of exactly this colour are transparent.
const KEY: COLORREF = rgb(255, 0, 254);

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

#[derive(Clone, Copy)]
pub(super) struct Palette {
    active: COLORREF,
    inactive: COLORREF,
    hover: COLORREF,
    text: COLORREF,
    glyph_hover: COLORREF,
    accent: COLORREF,
    /// Inactive tabs while the bar is tucked; the active one uses `accent`.
    sliver: COLORREF,
}

pub(super) const LIGHT: Palette = Palette {
    active: rgb(0xFF, 0xFF, 0xFF),
    inactive: rgb(0xE3, 0xE3, 0xE3),
    hover: rgb(0xEE, 0xEE, 0xEE),
    text: rgb(0x1A, 0x1A, 0x1A),
    glyph_hover: rgb(0xD0, 0xD0, 0xD0),
    accent: rgb(0x05, 0x96, 0x69),
    sliver: rgb(0xBD, 0xBD, 0xBD),
};

pub(super) const DARK: Palette = Palette {
    active: rgb(0x3A, 0x3A, 0x3A),
    inactive: rgb(0x1F, 0x1F, 0x1F),
    hover: rgb(0x2B, 0x2B, 0x2B),
    text: rgb(0xF2, 0xF2, 0xF2),
    glyph_hover: rgb(0x55, 0x55, 0x55),
    accent: rgb(0x10, 0xB9, 0x81),
    sliver: rgb(0x5C, 0x5C, 0x5C),
};

pub struct TabView {
    pub title: String,
    pub icon: HICON,
}

/// Everything needed to draw a bar's tabs, so the settings preview draws them exactly as a real
/// bar would.
pub(super) struct BarPicture<'a> {
    pub tabs: &'a [TabView],
    pub active: usize,
    pub geoms: &'a [TabGeom],
    pub metrics: &'a BarMetrics,
    pub palette: Palette,
    pub font: HFONT,
    pub hover: Hit,
    /// Draw a sliver of each tab instead of the tabs themselves.
    pub tucked: bool,
}

/// Where the bar goes in the z-order.
pub enum ZOrder {
    /// Already directly above the active window.
    Keep,
    /// Directly below this window, i.e. just above the active window.
    After(Win),
    /// Top of the normal (non-topmost) band, for when the active window is the top normal window.
    TopNormal,
}

struct Press {
    hit: Hit,
    x: i32,
    y: i32,
    dragging: bool,
}

struct State {
    controller: HWND,
    group: GroupId,
    base: BarMetrics,
    metrics: BarMetrics,
    dpi: u32,
    font: HFONT,
    palette: Palette,
    middle_click_closes: bool,
    tabs: Vec<TabView>,
    active: usize,
    geoms: Vec<TabGeom>,
    width: i32,
    /// Showing only a sliver of each tab along the top edge of a maximized window.
    tucked: bool,
    hover: Hit,
    tracking: bool,
    press: Option<Press>,
}

impl State {
    fn relayout(&mut self) {
        self.geoms = layout::layout_tabs(self.width, self.tabs.len(), &self.metrics);
        if let Hit::Tab(i) | Hit::Close(i) = self.hover
            && i >= self.tabs.len()
        {
            self.hover = Hit::None;
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if !self.font.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.font.into());
            }
        }
    }
}

pub struct TabBar {
    hwnd: HWND,
    // The window holds a non-owning pointer to this state; it is destroyed before the state.
    state: Rc<RefCell<State>>,
}

pub fn register_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

impl TabBar {
    pub fn new(
        instance: HINSTANCE,
        controller: HWND,
        group: GroupId,
        base: BarMetrics,
        light: bool,
        middle_click_closes: bool,
    ) -> windows::core::Result<Self> {
        let state = Rc::new(RefCell::new(State {
            controller,
            group,
            base,
            metrics: base,
            dpi: 0,
            font: HFONT::default(),
            palette: if light { LIGHT } else { DARK },
            middle_click_closes,
            tabs: Vec::new(),
            active: 0,
            geoms: Vec::new(),
            width: 0,
            tucked: false,
            hover: Hit::None,
            tracking: false,
            press: None,
        }));
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                CLASS,
                w!("Grouply tabs"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                Some(Rc::as_ptr(&state) as *const _),
            )?
        };
        let bar = Self { hwnd, state };
        unsafe { SetLayeredWindowAttributes(hwnd, KEY, 0, LWA_COLORKEY)? };
        Ok(bar)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn set_tabs(&self, tabs: Vec<TabView>, active: usize) {
        {
            let mut st = self.state.borrow_mut();
            st.tabs = tabs;
            st.active = active;
            st.relayout();
        }
        self.invalidate();
    }

    pub fn set_light(&self, light: bool) {
        self.state.borrow_mut().palette = if light { LIGHT } else { DARK };
        self.invalidate();
    }

    /// Applies changed settings to a bar that already exists.
    pub fn set_options(&self, base: BarMetrics, middle_click_closes: bool) {
        {
            let mut st = self.state.borrow_mut();
            st.base = base;
            st.metrics = if st.dpi == 0 {
                base
            } else {
                base.scaled(st.dpi)
            };
            st.middle_click_closes = middle_click_closes;
            st.relayout();
        }
        self.invalidate();
    }

    /// Whether the whole bar is showing, as opposed to hidden or tucked.
    pub fn is_expanded(&self) -> bool {
        win::is_visible(Win::from(self.hwnd)) && !self.state.borrow().tucked
    }

    /// Whether a tab is being pressed or dragged, so the bar must stay as it is.
    pub fn is_pressed(&self) -> bool {
        self.state.borrow().press.is_some()
    }

    /// Positions the bar for an active window with visible bounds `frame` and shows it, either
    /// whole or `tucked` to a sliver of each tab along the top of the work area.
    pub fn place(&self, frame: Rect, work_area: Rect, dpi: u32, z: ZOrder, tucked: bool) {
        let rect = {
            let mut st = self.state.borrow_mut();
            if st.dpi != dpi {
                st.dpi = dpi;
                st.metrics = st.base.scaled(dpi);
                let old = std::mem::replace(&mut st.font, make_font(dpi));
                if !old.is_invalid() {
                    unsafe {
                        let _ = DeleteObject(old.into());
                    }
                }
            }
            let rect = if tucked {
                layout::tucked_rect(frame, work_area, &st.metrics)
            } else {
                layout::bar_rect(frame, work_area, &st.metrics)
            };
            st.width = rect.width();
            st.tucked = tucked;
            if tucked {
                st.hover = Hit::None;
            }
            st.relayout();
            rect
        };
        // Slivers are only a hint: clicks go through to the window underneath.
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let transparent = WS_EX_TRANSPARENT.0 as isize;
            let want = if tucked {
                ex | transparent
            } else {
                ex & !transparent
            };
            if want != ex {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, want);
            }
        }
        let mut flags = SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER;
        let after = match z {
            ZOrder::Keep => {
                flags |= SWP_NOZORDER;
                None
            }
            ZOrder::After(w) => Some(w.hwnd()),
            ZOrder::TopNormal => Some(HWND_NOTOPMOST),
        };
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                after,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                flags,
            )
        };
        self.invalidate();
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Screen bounds while the bar is showing.
    pub fn visible_rect(&self) -> Option<Rect> {
        let me = Win::from(self.hwnd);
        if !win::is_visible(me) {
            return None;
        }
        win::frame(me)
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}

impl Drop for TabBar {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

pub(super) fn make_font(dpi: u32) -> HFONT {
    // 9 pt, the size of standard window captions.
    let height = -(12 * dpi as i32 / 96);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
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
    }
}

fn post(controller: HWND, group: GroupId, cmd: TabCommand) {
    let _ = unsafe {
        PostMessageW(
            Some(controller),
            WM_APP_TAB,
            WPARAM(group as usize),
            LPARAM(cmd.encode() as isize),
        )
    };
}

/// Client coordinates from a mouse message; negative while the mouse is captured outside.
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
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // Valid until WM_NCDESTROY: TabBar destroys the window before dropping the state.
        let cell = &*ptr;
        match msg {
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd, cell);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                on_mouse_move(hwnd, cell, point(lparam));
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if let Ok(mut st) = cell.try_borrow_mut() {
                    st.tracking = false;
                    st.hover = Hit::None;
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                on_left_down(hwnd, cell, point(lparam));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                on_left_up(cell, point(lparam));
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                if let Ok(mut st) = cell.try_borrow_mut() {
                    st.press = None;
                }
                LRESULT(0)
            }
            WM_MBUTTONUP => {
                on_middle_up(cell, point(lparam));
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                on_right_up(cell, point(lparam));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// Mouse handlers decide inside a short borrow and act after it ends: capture changes and menus
// re-enter the window procedure synchronously.

fn on_mouse_move(hwnd: HWND, cell: &RefCell<State>, (x, y): (i32, i32)) {
    let threshold = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(4);
    let (track, changed) = {
        let Ok(mut st) = cell.try_borrow_mut() else {
            return;
        };
        if let Some(press) = st.press.as_mut()
            && matches!(press.hit, Hit::Tab(_))
            && ((x - press.x).abs() > threshold || (y - press.y).abs() > threshold)
        {
            press.dragging = true;
        }
        let hit = layout::hit_test(&st.geoms, x, y);
        let changed = hit != st.hover;
        st.hover = hit;
        let track = !st.tracking;
        st.tracking = true;
        (track, changed)
    };
    if track {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let _ = unsafe { TrackMouseEvent(&mut tme) };
    }
    if changed {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

fn on_left_down(hwnd: HWND, cell: &RefCell<State>, (x, y): (i32, i32)) {
    let (target, send) = {
        let Ok(mut st) = cell.try_borrow_mut() else {
            return;
        };
        let hit = layout::hit_test(&st.geoms, x, y);
        st.press = Some(Press {
            hit,
            x,
            y,
            dragging: false,
        });
        let send = match hit {
            Hit::Tab(i) => Some(TabCommand::Activate(i)),
            _ => None,
        };
        ((st.controller, st.group), send)
    };
    unsafe { SetCapture(hwnd) };
    if let Some(cmd) = send {
        post(target.0, target.1, cmd);
    }
}

fn on_left_up(cell: &RefCell<State>, (x, y): (i32, i32)) {
    let action = {
        let Ok(mut st) = cell.try_borrow_mut() else {
            return;
        };
        st.press.take().and_then(|press| {
            let cmd = match press.hit {
                Hit::Close(i) if layout::hit_test(&st.geoms, x, y) == Hit::Close(i) => {
                    Some(TabCommand::Close(i))
                }
                Hit::Tab(i) if press.dragging => {
                    let h = st.metrics.height;
                    let outside = y < -h || y > 2 * h || x < -h || x > st.width + h;
                    if outside {
                        Some(TabCommand::Detach(i))
                    } else {
                        let to = layout::reorder_target(&st.geoms, i, x);
                        (to != i).then_some(TabCommand::Reorder { from: i, to })
                    }
                }
                _ => None,
            };
            cmd.map(|c| (st.controller, st.group, c))
        })
    };
    let _ = unsafe { ReleaseCapture() };
    if let Some((controller, group, cmd)) = action {
        post(controller, group, cmd);
    }
}

fn on_middle_up(cell: &RefCell<State>, (x, y): (i32, i32)) {
    let action = {
        let Ok(st) = cell.try_borrow() else { return };
        match layout::hit_test(&st.geoms, x, y) {
            Hit::Tab(i) | Hit::Close(i) if st.middle_click_closes => {
                Some((st.controller, st.group, i))
            }
            _ => None,
        }
    };
    if let Some((controller, group, i)) = action {
        post(controller, group, TabCommand::Close(i));
    }
}

fn on_right_up(cell: &RefCell<State>, (x, y): (i32, i32)) {
    let (controller, group, index, active) = {
        let Ok(st) = cell.try_borrow() else { return };
        match layout::hit_test(&st.geoms, x, y) {
            Hit::Tab(i) | Hit::Close(i) => (st.controller, st.group, i, st.active),
            Hit::None => return,
        }
    };
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    let choice = unsafe {
        let _ = AppendMenuW(menu, MF_STRING, 1, w!("Close tab"));
        let _ = AppendMenuW(menu, MF_STRING, 2, w!("Remove from group"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, 3, w!("Ungroup all"));
        let _ = AppendMenuW(menu, MF_STRING, 4, w!("Close all windows in group"));
        let (sx, sy) = win::cursor_pos();
        // The bar never activates, so the hidden controller owns the menu; it must be
        // foreground for the menu to dismiss when the user clicks elsewhere.
        let _ = SetForegroundWindow(controller);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            sx,
            sy,
            None,
            controller,
            None,
        );
        let _ = PostMessageW(Some(controller), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        choice.0
    };
    let cmd = match choice {
        1 => TabCommand::Close(index),
        2 => TabCommand::Detach(index),
        3 => TabCommand::Ungroup,
        4 => TabCommand::CloseAll,
        // Dismissed: hand focus back to the group.
        _ => TabCommand::Activate(active),
    };
    post(controller, group, cmd);
}

fn paint(hwnd: HWND, cell: &RefCell<State>) {
    let mut ps = PAINTSTRUCT::default();
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        if let Ok(st) = cell.try_borrow() {
            draw(hwnd, hdc, &st);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

fn to_win_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

unsafe fn draw(hwnd: HWND, hdc: HDC, st: &State) {
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client).is_err() || client.right <= 0 || client.bottom <= 0 {
            return;
        }
        let (w, h) = (client.right, client.bottom);
        let mem = CreateCompatibleDC(Some(hdc));
        let bitmap = CreateCompatibleBitmap(hdc, w, h);
        let old_bitmap = SelectObject(mem, bitmap.into());
        fill(mem, client, KEY);

        paint_tabs(
            mem,
            &BarPicture {
                tabs: &st.tabs,
                active: st.active,
                geoms: &st.geoms,
                metrics: &st.metrics,
                palette: st.palette,
                font: st.font,
                hover: st.hover,
                tucked: st.tucked,
            },
            h,
        );

        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
    }
}

/// Draws a bar's tabs with the bar's top-left corner at the DC's origin. `height` is the height
/// of the area being painted: the whole bar, or a tucked sliver.
pub(super) unsafe fn paint_tabs(hdc: HDC, pic: &BarPicture, h: i32) {
    unsafe {
        let old_font = SelectObject(hdc, pic.font.into());
        SetBkMode(hdc, TRANSPARENT);
        let p = pic.palette;
        let m = pic.metrics;
        if pic.tucked {
            for (i, g) in pic.geoms.iter().enumerate() {
                let color = if i == pic.active { p.accent } else { p.sliver };
                sliver_shape(hdc, g.tab, h, m.corner, color);
            }
        } else {
            for (i, (tab, g)) in pic.tabs.iter().zip(pic.geoms).enumerate() {
                let active = i == pic.active;
                let hovered = matches!(pic.hover, Hit::Tab(j) | Hit::Close(j) if j == i);
                let color = if active {
                    p.active
                } else if hovered {
                    p.hover
                } else {
                    p.inactive
                };
                tab_shape(hdc, g.tab, m.corner, color);
                if active {
                    let stripe = (m.height / 16).max(2);
                    let r = Rect::new(
                        g.tab.left + m.corner / 2,
                        g.tab.top,
                        g.tab.right - m.corner / 2,
                        g.tab.top + stripe,
                    );
                    fill(hdc, to_win_rect(r), p.accent);
                }
                if !tab.icon.is_invalid() {
                    let _ = DrawIconEx(
                        hdc,
                        g.icon.left,
                        g.icon.top,
                        tab.icon,
                        g.icon.width(),
                        g.icon.height(),
                        0,
                        None,
                        DI_NORMAL,
                    );
                }
                SetTextColor(hdc, p.text);
                let mut text: Vec<u16> = tab.title.encode_utf16().collect();
                let mut r = to_win_rect(g.text);
                DrawTextW(
                    hdc,
                    &mut text,
                    &mut r,
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | DT_LEFT,
                );
                if active || hovered {
                    close_glyph(hdc, g.close, pic.hover == Hit::Close(i), &p, m);
                }
            }
        }
        SelectObject(hdc, old_font);
    }
}

unsafe fn fill(hdc: HDC, r: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(hdc, &r, brush);
        let _ = DeleteObject(brush.into());
    }
}

unsafe fn tab_shape(hdc: HDC, r: Rect, corner: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let pen = CreatePen(PS_SOLID, 1, color);
        let old_brush = SelectObject(hdc, brush.into());
        let old_pen = SelectObject(hdc, pen.into());
        // Running past the bitmap's bottom edge clips the lower corners, leaving them square.
        let _ = RoundRect(
            hdc,
            r.left,
            r.top,
            r.right,
            r.bottom + corner,
            corner,
            corner,
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

/// The bottom `height` pixels of a tab's span, with rounded lower corners.
unsafe fn sliver_shape(hdc: HDC, tab: Rect, height: i32, corner: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let pen = CreatePen(PS_SOLID, 1, color);
        let old_brush = SelectObject(hdc, brush.into());
        let old_pen = SelectObject(hdc, pen.into());
        let round = corner.min(height);
        // Starting above the bitmap clips the upper corners, leaving them square.
        let _ = RoundRect(hdc, tab.left, -height, tab.right, height, round, round);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

unsafe fn close_glyph(hdc: HDC, r: Rect, hot: bool, p: &Palette, m: &BarMetrics) {
    unsafe {
        if hot {
            let brush = CreateSolidBrush(p.glyph_hover);
            let pen = CreatePen(PS_SOLID, 1, p.glyph_hover);
            let old_brush = SelectObject(hdc, brush.into());
            let old_pen = SelectObject(hdc, pen.into());
            let _ = Ellipse(hdc, r.left, r.top, r.right, r.bottom);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush.into());
            let _ = DeleteObject(pen.into());
        }
        let inset = r.width() / 4;
        let pen = CreatePen(PS_SOLID, (m.close / 10).max(1), p.text);
        let old_pen = SelectObject(hdc, pen.into());
        let _ = MoveToEx(hdc, r.left + inset, r.top + inset, None);
        let _ = LineTo(hdc, r.right - inset, r.bottom - inset);
        let _ = MoveToEx(hdc, r.right - inset - 1, r.top + inset, None);
        let _ = LineTo(hdc, r.left + inset - 1, r.bottom - inset);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(pen.into());
    }
}
