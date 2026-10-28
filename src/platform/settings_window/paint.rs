//! Drawing the settings window: the pinned preview, then the page of controls under it.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx};

use super::super::tabbar::{self, BarPicture};
use super::canvas::Canvas;
use super::{State, cref, track};
use crate::layout::{self, BarMetrics, Hit, Rect};
use crate::settings::{Field, MaximizedBar};
use crate::settings_ui::{self, Control, PRESETS, Section, Toggle};

/// The picture inside a preset or mode card.
pub(super) fn mini_rect(r: Rect, dpi: u32, height: i32) -> Rect {
    let s = |v: i32| v * dpi as i32 / 96;
    Rect::new(
        r.left + s(8),
        r.top + s(8),
        r.right - s(8),
        r.top + s(8) + s(height),
    )
}

/// Where a dial's knob sits in its cell.
fn dial_center(r: Rect, dpi: u32) -> (f32, f32) {
    let s = |v: i32| v * dpi as i32 / 96;
    ((r.left + r.right) as f32 / 2.0, (r.top + s(38)) as f32)
}

unsafe fn text(hdc: HDC, s: &str, r: Rect, font: HFONT, color: u32, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        SelectObject(hdc, font.into());
        SetTextColor(hdc, cref(color));
        let mut buf: Vec<u16> = s.encode_utf16().collect();
        let mut rc = RECT {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        DrawTextW(
            hdc,
            &mut buf,
            &mut rc,
            flags | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
    }
}

unsafe fn fill(hdc: HDC, r: Rect, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(cref(color));
        let rc = RECT {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        FillRect(hdc, &rc, brush);
        let _ = DeleteObject(brush.into());
    }
}

unsafe fn gdi_line(hdc: HDC, from: (i32, i32), to: (i32, i32), color: u32, width: i32) {
    unsafe {
        let pen = CreatePen(PS_SOLID, width.max(1), cref(color));
        let old = SelectObject(hdc, pen.into());
        let _ = MoveToEx(hdc, from.0, from.1, None);
        let _ = LineTo(hdc, to.0, to.1);
        SelectObject(hdc, old);
        let _ = DeleteObject(pen.into());
    }
}

/// A vertical gradient, top colour to bottom colour.
unsafe fn gradient(hdc: HDC, r: Rect, (top, bottom): (u32, u32)) {
    let vertex = |x: i32, y: i32, c: u32| TRIVERTEX {
        x,
        y,
        Red: ((c >> 16) & 0xFF) as u16 * 256,
        Green: ((c >> 8) & 0xFF) as u16 * 256,
        Blue: (c & 0xFF) as u16 * 256,
        Alpha: 0xFF00,
    };
    let vertices = [
        vertex(r.left, r.top, top),
        vertex(r.right, r.bottom, bottom),
    ];
    let mesh = GRADIENT_RECT {
        UpperLeft: 0,
        LowerRight: 1,
    };
    unsafe {
        let _ = GradientFill(
            hdc,
            &vertices,
            &mesh as *const GRADIENT_RECT as *const c_void,
            1,
            GRADIENT_FILL_RECT_V,
        );
    }
}

pub(super) fn paint(hwnd: HWND, cell: &RefCell<State>) {
    let mut ps = PAINTSTRUCT::default();
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        if let Ok(st) = cell.try_borrow() {
            draw(hdc, &st);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe fn draw(hdc: HDC, st: &State) {
    let (w, h) = st.client;
    if w <= 0 || h <= 0 {
        return;
    }
    unsafe {
        let mem = CreateCompatibleDC(Some(hdc));
        let bitmap = CreateCompatibleBitmap(hdc, w, h);
        let old_bitmap = SelectObject(mem, bitmap.into());
        let old_font = SelectObject(mem, st.fonts.body.into());
        SetBkMode(mem, TRANSPARENT);
        fill(mem, Rect::new(0, 0, w, h), st.theme.bg);
        draw_page(mem, st);
        // The header goes on last: the page scrolls underneath it.
        draw_header(mem, st);
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_font);
        SelectObject(mem, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
    }
}

unsafe fn draw_header(mem: HDC, st: &State) {
    let t = &st.theme;
    let l = &st.layout;
    let s = |v: i32| st.s(v);
    unsafe {
        fill(mem, Rect::new(0, 0, st.client.0, l.header), t.bg);
        let p = l.preview;
        let saved = SaveDC(mem);
        IntersectClipRect(mem, p.left, p.top, p.right, p.bottom);
        gradient(mem, p, t.desk);
        // A normal window with its bar above it, as on the desktop.
        let m = st.settings.metrics.scaled(st.dpi);
        let frame = Rect::new(
            p.left + s(20),
            p.top + s(104),
            p.right - s(20),
            p.bottom + s(60),
        );
        app_window(mem, st, frame);
        draw_bar(mem, st, &m, layout::bar_rect(frame, p, &m), false);
        let _ = RestoreDC(mem, saved);
        if st.scroll > 0 {
            gdi_line(
                mem,
                (0, l.header - 1),
                (st.client.0, l.header - 1),
                t.border,
                1,
            );
        }
        if let Some(c) = Canvas::new(mem, Rect::new(0, 0, st.client.0, l.header)) {
            c.mask_corners(p, s(8) as f32, t.bg);
            c.stroke_round(p, s(8) as f32, t.border, 1.0);
        }
    }
}

/// A plain window frame: caption with icon, title and caption buttons over an empty client.
unsafe fn app_window(mem: HDC, st: &State, frame: Rect) {
    let t = &st.theme;
    let s = |v: i32| st.s(v);
    unsafe {
        fill(mem, frame, t.window);
        let caption = Rect::new(frame.left, frame.top, frame.right, frame.top + s(32));
        fill(mem, caption, t.caption);
        if let Some(tab) = st.sample.0.first() {
            let _ = DrawIconEx(
                mem,
                frame.left + s(12),
                frame.top + s(8),
                tab.icon,
                s(16),
                s(16),
                0,
                None,
                DI_NORMAL,
            );
            let title = Rect::new(
                frame.left + s(40),
                caption.top,
                frame.right - s(150),
                caption.bottom,
            );
            text(
                mem,
                &tab.title,
                title,
                st.fonts.small,
                t.text,
                DT_VCENTER | DT_LEFT,
            );
        }
        // Minimize, maximize and close glyphs in 46 px cells.
        let cy = caption.top + s(16);
        let cell = |i: i32| frame.right - s(46) * (3 - i) + s(18);
        let g = s(10);
        gdi_line(mem, (cell(0), cy), (cell(0) + g, cy), t.text, 1);
        let x = cell(1);
        for (a, b) in [
            ((x, cy - g / 2), (x + g, cy - g / 2)),
            ((x + g, cy - g / 2), (x + g, cy + g / 2)),
            ((x + g, cy + g / 2), (x, cy + g / 2)),
            ((x, cy + g / 2), (x, cy - g / 2)),
        ] {
            gdi_line(mem, a, b, t.text, 1);
        }
        let x = cell(2);
        gdi_line(mem, (x, cy - g / 2), (x + g, cy + g / 2), t.text, 1);
        gdi_line(mem, (x + g, cy - g / 2), (x, cy + g / 2), t.text, 1);
        gdi_line(
            mem,
            (frame.left, caption.bottom),
            (frame.right, caption.bottom),
            t.border,
            1,
        );
    }
}

/// Draws the sample tabs into `bar` with the tab bar's own painting.
unsafe fn draw_bar(mem: HDC, st: &State, m: &BarMetrics, bar: Rect, tucked: bool) {
    let geoms = layout::layout_tabs(bar.width(), st.sample.0.len(), m);
    let picture = BarPicture {
        tabs: &st.sample.0,
        active: 0,
        geoms: &geoms,
        metrics: m,
        palette: if st.theme.light {
            tabbar::LIGHT
        } else {
            tabbar::DARK
        },
        font: st.fonts.tab,
        hover: Hit::None,
        tucked,
    };
    unsafe {
        let saved = SaveDC(mem);
        IntersectClipRect(mem, bar.left, bar.top, bar.right, bar.bottom);
        let _ = SetViewportOrgEx(mem, bar.left, bar.top, None);
        tabbar::paint_tabs(mem, &picture, bar.height());
        let _ = RestoreDC(mem, saved);
    }
}

unsafe fn draw_page(mem: HDC, st: &State) {
    let l = &st.layout;
    let (w, h) = st.client;
    let page = Rect::new(0, l.header, w, h);
    let at = |r: Rect| r.offset(0, -st.scroll);
    unsafe {
        let saved = SaveDC(mem);
        IntersectClipRect(mem, page.left, page.top, page.right, page.bottom);
        if let Some(c) = Canvas::new(mem, page) {
            for &(control, r) in &l.controls {
                draw_shape(&c, st, control, at(r));
            }
        }
        for &(section, r) in &l.sections {
            draw_heading(mem, st, section, at(r));
        }
        for &(control, r) in &l.controls {
            draw_content(mem, st, control, at(r));
        }
        // GDI-drawn pictures inside cards get rounded corners last.
        if let Some(c) = Canvas::new(mem, page) {
            for &(control, r) in &l.controls {
                let mini = match control {
                    Control::Preset(_) => mini_rect(at(r), st.dpi, 44),
                    Control::Mode(_) => mini_rect(at(r), st.dpi, 36),
                    _ => continue,
                };
                c.mask_corners(mini, st.s(4) as f32, card_fill(st, control));
            }
        }
        let _ = RestoreDC(mem, saved);
    }
}

fn selected(st: &State, control: Control) -> bool {
    match control {
        Control::Preset(i) => PRESETS[i].matches(&st.settings.metrics),
        Control::Mode(mode) => st.settings.maximized == mode,
        Control::Toggle(toggle) => st.toggle(toggle),
        _ => false,
    }
}

fn card_fill(st: &State, control: Control) -> u32 {
    let t = &st.theme;
    if matches!(control, Control::Toggle(_)) && selected(st, control) {
        t.selected
    } else if st.hover == Some(control) || st.pressed == Some(control) {
        t.hover
    } else {
        t.card
    }
}

fn draw_shape(c: &Canvas, st: &State, control: Control, r: Rect) {
    let t = &st.theme;
    let s = |v: i32| st.s(v) as f32;
    let hovered = st.hover == Some(control);
    let radius = s(8);
    match control {
        Control::Preset(_) | Control::Mode(_) | Control::Toggle(_) => {
            let (border, width) = if selected(st, control) {
                (t.accent, s(2))
            } else {
                (t.border, 1.0)
            };
            c.card(r, radius, card_fill(st, control), border, width);
            if let Control::Toggle(toggle) = control {
                // A small switch on the right.
                let on = st.toggle(toggle);
                let (w, h) = (s(36), s(18));
                let x = r.right as f32 - s(14) - w;
                let y = (r.top + r.bottom) as f32 / 2.0 - h / 2.0;
                let pill = Rect::new(x as i32, y as i32, (x + w) as i32, (y + h) as i32);
                if on {
                    c.fill_round(pill, h / 2.0, t.accent);
                } else {
                    c.stroke_round(pill, h / 2.0, t.muted, s(1));
                }
                let knob_x = if on { x + w - h / 2.0 } else { x + h / 2.0 };
                let knob = if on { 0xFFFFFF } else { t.muted };
                c.circle(knob_x, y + h / 2.0, s(5), knob);
            }
        }
        Control::Dial(field) => {
            let (cx, cy) = dial_center(r, st.dpi);
            let value = field.get(&st.settings);
            let p = settings_ui::fraction(field, value);
            let active = hovered || st.drag.as_ref().is_some_and(|d| d.field == field);
            let ring = s(30);
            c.circle(cx, cy, s(24), if active { t.hover } else { t.card });
            c.arc((cx, cy), ring, (135.0, 270.0), t.track, s(5));
            if p > 0.0 {
                c.arc((cx, cy), ring, (135.0, 270.0 * p), t.accent, s(5));
            }
            let a = (135.0 + 270.0 * p).to_radians();
            let (dx, dy) = (cx + ring * a.cos(), cy + ring * a.sin());
            c.circle(dx, dy, s(6), t.bg);
            c.circle(dx, dy, s(4), t.accent);
        }
        Control::Slider(field) => {
            let (left, right, y) = track(r, st.dpi);
            let p = settings_ui::fraction(field, field.get(&st.settings));
            let x = left as f32 + (right - left) as f32 * p;
            let y = y as f32;
            c.line((left as f32, y), (right as f32, y), t.track, s(4));
            if p > 0.0 {
                c.line((left as f32, y), (x, y), t.accent, s(4));
            }
            let active = hovered || st.drag.as_ref().is_some_and(|d| d.field == field);
            c.circle(x, y, s(10), t.border);
            c.circle(x, y, s(9), t.card);
            c.circle(x, y, if active { s(6) } else { s(5) }, t.accent);
        }
        Control::More => {
            if hovered {
                c.fill_round(r, s(6), t.hover);
            }
            // A chevron at the right, pointing down when closed and up when open.
            let cx = r.right as f32 - s(16);
            let cy = (r.top + r.bottom) as f32 / 2.0;
            let d = if st.more { -s(3) } else { s(3) };
            c.line((cx - s(5), cy - d), (cx, cy + d), t.text, s(1) * 1.5);
            c.line((cx, cy + d), (cx + s(5), cy - d), t.text, s(1) * 1.5);
        }
        Control::Reset | Control::OpenFile => {
            c.card(r, s(6), card_fill(st, control), t.border, 1.0);
        }
    }
    if st.cues && st.focus == Some(control) {
        let ring = Rect::new(r.left - 3, r.top - 3, r.right + 3, r.bottom + 3);
        c.stroke_round(ring, radius + 3.0, t.text, s(2));
    }
}

unsafe fn draw_heading(mem: HDC, st: &State, section: Section, r: Rect) {
    let t = &st.theme;
    let title = match section {
        Section::Presets => "Start from a preset",
        Section::FineTune => "Fine-tune",
        Section::Behaviour => "Behaviour",
        Section::Maximized => "Maximized windows",
    };
    unsafe {
        text(
            mem,
            title,
            r,
            st.fonts.heading,
            t.text,
            DT_LEFT | DT_VCENTER,
        );
        if section == Section::FineTune {
            let hint = "Drag, scroll or type a number. Double-click resets.";
            text(mem, hint, r, st.fonts.small, t.muted, DT_RIGHT | DT_VCENTER);
        }
    }
}

unsafe fn draw_content(mem: HDC, st: &State, control: Control, r: Rect) {
    let t = &st.theme;
    let f = &st.fonts;
    let s = |v: i32| st.s(v);
    let typed = |field: Field| match (&st.typing, st.focus) {
        (Some(digits), Some(Control::Dial(x) | Control::Slider(x))) if x == field => {
            Some(format!("{digits}_"))
        }
        _ => None,
    };
    unsafe {
        match control {
            Control::Preset(i) => {
                let mini = mini_rect(r, st.dpi, 44);
                let m = PRESETS[i].apply(st.settings.metrics).scaled(st.dpi);
                let saved = SaveDC(mem);
                IntersectClipRect(mem, mini.left, mini.top, mini.right, mini.bottom);
                gradient(mem, mini, t.desk);
                let frame = Rect::new(
                    mini.left + s(6),
                    mini.bottom - s(6),
                    mini.left + s(600),
                    mini.bottom + s(60),
                );
                app_window(mem, st, frame);
                // A tall virtual work area keeps the bar above the frame, as on the desktop.
                let work = Rect::new(frame.left, frame.top - s(200), frame.right, frame.bottom);
                draw_bar(mem, st, &m, layout::bar_rect(frame, work, &m), false);
                let _ = RestoreDC(mem, saved);
                let label = Rect::new(
                    r.left + s(10),
                    mini.bottom + s(2),
                    r.right - s(6),
                    r.bottom - s(2),
                );
                text(
                    mem,
                    PRESETS[i].name,
                    label,
                    f.small,
                    t.text,
                    DT_LEFT | DT_VCENTER,
                );
            }
            Control::Mode(mode) => {
                let mini = mini_rect(r, st.dpi, 36);
                let m = st.settings.metrics.scaled(st.dpi);
                let saved = SaveDC(mem);
                IntersectClipRect(mem, mini.left, mini.top, mini.right, mini.bottom);
                // The left end of a maximized window's caption, as the bar would cover it.
                let frame = Rect::new(
                    mini.left,
                    mini.top,
                    mini.left + s(640),
                    mini.bottom + s(100),
                );
                app_window(mem, st, frame);
                match mode {
                    MaximizedBar::Always => {
                        draw_bar(mem, st, &m, layout::bar_rect(frame, frame, &m), false)
                    }
                    MaximizedBar::Tucked => {
                        draw_bar(mem, st, &m, layout::tucked_rect(frame, frame, &m), true)
                    }
                    MaximizedBar::Hidden => {}
                }
                let _ = RestoreDC(mem, saved);
                let name = match mode {
                    MaximizedBar::Always => "Always on",
                    MaximizedBar::Tucked => "Tucked",
                    MaximizedBar::Hidden => "Hidden",
                };
                let label = Rect::new(
                    r.left + s(10),
                    mini.bottom + s(2),
                    r.right - s(6),
                    r.bottom - s(2),
                );
                text(mem, name, label, f.small, t.text, DT_LEFT | DT_VCENTER);
            }
            Control::Dial(field) => {
                let (cx, cy) = dial_center(r, st.dpi);
                let (cx, cy) = (cx as i32, cy as i32);
                let value = typed(field).unwrap_or_else(|| field.get(&st.settings).to_string());
                let center = Rect::new(cx - s(24), cy - s(12), cx + s(24), cy + s(12));
                text(mem, &value, center, f.value, t.text, DT_CENTER | DT_VCENTER);
                let label = Rect::new(r.left, r.top + s(76), r.right, r.top + s(94));
                text(
                    mem,
                    settings_ui::label(field),
                    label,
                    f.body,
                    t.text,
                    DT_CENTER | DT_VCENTER,
                );
                let unit = Rect::new(r.left, r.top + s(92), r.right, r.bottom);
                text(
                    mem,
                    settings_ui::unit(field),
                    unit,
                    f.small,
                    t.muted,
                    DT_CENTER | DT_VCENTER,
                );
            }
            Control::Toggle(toggle) => {
                let title = match toggle {
                    Toggle::MiddleClick => "Middle-click closes",
                    Toggle::CycleHotkeys => "Win + ` switches tabs",
                    Toggle::Autostart => "Start with Windows",
                };
                let on = st.toggle(toggle);
                let line1 = Rect::new(r.left + s(12), r.top + s(9), r.right - s(56), r.top + s(29));
                text(mem, title, line1, f.strong, t.text, DT_LEFT | DT_VCENTER);
                let line2 = Rect::new(
                    r.left + s(12),
                    r.top + s(29),
                    r.right - s(56),
                    r.bottom - s(8),
                );
                let (state, color) = if on {
                    ("On", t.accent)
                } else {
                    ("Off", t.muted)
                };
                text(mem, state, line2, f.small, color, DT_LEFT | DT_VCENTER);
            }
            Control::More => {
                let title = if st.more { "Fewer sizes" } else { "More sizes" };
                let line = Rect::new(r.left + s(4), r.top, r.right - s(32), r.bottom);
                text(mem, title, line, f.strong, t.text, DT_LEFT | DT_VCENTER);
                if !st.more {
                    let hint = "Space above tabs, inset, padding, icon and close sizes";
                    text(mem, hint, line, f.small, t.muted, DT_RIGHT | DT_VCENTER);
                }
            }
            Control::Slider(field) => {
                let head = Rect::new(r.left + s(4), r.top, r.right - s(4), r.top + s(20));
                text(
                    mem,
                    settings_ui::label(field),
                    head,
                    f.body,
                    t.text,
                    DT_LEFT | DT_VCENTER,
                );
                let value = typed(field).unwrap_or_else(|| {
                    format!("{} {}", field.get(&st.settings), settings_ui::unit(field))
                });
                text(mem, &value, head, f.small, t.muted, DT_RIGHT | DT_VCENTER);
            }
            Control::Reset => text(
                mem,
                "Reset to defaults",
                r,
                f.body,
                t.text,
                DT_CENTER | DT_VCENTER,
            ),
            Control::OpenFile => text(
                mem,
                "Open settings file",
                r,
                f.body,
                t.text,
                DT_CENTER | DT_VCENTER,
            ),
        }
    }
}
