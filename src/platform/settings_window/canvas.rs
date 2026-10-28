//! Anti-aliased shapes for the settings window, drawn with GDI+ over a GDI device context.

use std::ptr::null_mut;
use std::sync::OnceLock;

use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::GdiPlus as gp;

use super::argb;
use crate::layout::Rect;

/// Starts GDI+ once per process.
fn gdiplus_ready() -> bool {
    static STARTED: OnceLock<bool> = OnceLock::new();
    *STARTED.get_or_init(|| {
        let mut token = 0usize;
        let input = gp::GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let status = unsafe { gp::GdiplusStartup(&mut token, &input, null_mut()) };
        status.0 == 0
    })
}

/// Anti-aliased shapes over a GDI device context. Coordinates are client pixels.
pub(super) struct Canvas(*mut gp::GpGraphics);

impl Canvas {
    pub(super) fn new(hdc: HDC, clip: Rect) -> Option<Self> {
        if !gdiplus_ready() {
            return None;
        }
        let mut g = null_mut();
        unsafe {
            if gp::GdipCreateFromHDC(hdc, &mut g).0 != 0 || g.is_null() {
                return None;
            }
            gp::GdipSetSmoothingMode(g, gp::SmoothingModeAntiAlias8x4);
            gp::GdipSetPixelOffsetMode(g, gp::PixelOffsetModeHalf);
            gp::GdipSetClipRectI(
                g,
                clip.left,
                clip.top,
                clip.width(),
                clip.height(),
                gp::CombineModeReplace,
            );
        }
        Some(Self(g))
    }

    fn with_brush(&self, color: u32, f: impl FnOnce(*mut gp::GpBrush)) {
        let mut brush = null_mut();
        unsafe {
            if gp::GdipCreateSolidFill(argb(color), &mut brush).0 == 0 {
                f(brush as *mut gp::GpBrush);
                gp::GdipDeleteBrush(brush as *mut gp::GpBrush);
            }
        }
    }

    fn with_pen(&self, color: u32, width: f32, f: impl FnOnce(*mut gp::GpPen)) {
        let mut pen = null_mut();
        unsafe {
            if gp::GdipCreatePen1(argb(color), width, gp::UnitPixel, &mut pen).0 == 0 {
                gp::GdipSetPenStartCap(pen, gp::LineCapRound);
                gp::GdipSetPenEndCap(pen, gp::LineCapRound);
                f(pen);
                gp::GdipDeletePen(pen);
            }
        }
    }

    /// A rounded rectangle path; square when `radius` is under a pixel.
    fn round_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> *mut gp::GpPath {
        let mut path = null_mut();
        unsafe {
            gp::GdipCreatePath(gp::FillModeAlternate, &mut path);
            let d = (2.0 * radius).min(w).min(h);
            if d < 1.0 {
                gp::GdipAddPathLine(path, x, y, x + w, y);
                gp::GdipAddPathLine(path, x + w, y, x + w, y + h);
                gp::GdipAddPathLine(path, x + w, y + h, x, y + h);
            } else {
                gp::GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
                gp::GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
                gp::GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
                gp::GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
            }
            gp::GdipClosePathFigure(path);
        }
        path
    }

    pub(super) fn fill_round(&self, r: Rect, radius: f32, color: u32) {
        let path = Self::round_path(
            r.left as f32,
            r.top as f32,
            r.width() as f32,
            r.height() as f32,
            radius,
        );
        self.with_brush(color, |brush| unsafe {
            gp::GdipFillPath(self.0, brush, path);
        });
        unsafe {
            gp::GdipDeletePath(path);
        }
    }

    /// Strokes inside `r`, so the border doesn't spill past it.
    pub(super) fn stroke_round(&self, r: Rect, radius: f32, color: u32, width: f32) {
        let half = width / 2.0;
        let path = Self::round_path(
            r.left as f32 + half,
            r.top as f32 + half,
            r.width() as f32 - width,
            r.height() as f32 - width,
            (radius - half).max(0.0),
        );
        self.with_pen(color, width, |pen| unsafe {
            gp::GdipDrawPath(self.0, pen, path);
        });
        unsafe {
            gp::GdipDeletePath(path);
        }
    }

    /// A card: rounded fill with a border.
    pub(super) fn card(&self, r: Rect, radius: f32, fill: u32, border: u32, width: f32) {
        self.fill_round(r, radius, fill);
        self.stroke_round(r, radius, border, width);
    }

    pub(super) fn arc(
        &self,
        (cx, cy): (f32, f32),
        radius: f32,
        sweep: (f32, f32),
        color: u32,
        width: f32,
    ) {
        let (start, sweep) = sweep;
        self.with_pen(color, width, |pen| unsafe {
            gp::GdipDrawArc(
                self.0,
                pen,
                cx - radius,
                cy - radius,
                2.0 * radius,
                2.0 * radius,
                start,
                sweep,
            );
        });
    }

    pub(super) fn circle(&self, cx: f32, cy: f32, radius: f32, color: u32) {
        self.with_brush(color, |brush| unsafe {
            gp::GdipFillEllipse(
                self.0,
                brush,
                cx - radius,
                cy - radius,
                2.0 * radius,
                2.0 * radius,
            );
        });
    }

    pub(super) fn line(&self, from: (f32, f32), to: (f32, f32), color: u32, width: f32) {
        self.with_pen(color, width, |pen| unsafe {
            gp::GdipDrawLine(self.0, pen, from.0, from.1, to.0, to.1);
        });
    }

    /// Rounds a GDI-drawn picture's corners by painting `color` outside quarter circles.
    pub(super) fn mask_corners(&self, r: Rect, radius: f32, color: u32) {
        let (l, t, rt, b) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
        let d = 2.0 * radius;
        // Each corner: two straight edges along the rectangle, then the arc back.
        let corners = [
            ((l, t + radius), (l, t), (l + radius, t), (l, t), 270.0),
            (
                (rt - radius, t),
                (rt, t),
                (rt, t + radius),
                (rt - d, t),
                0.0,
            ),
            (
                (rt, b - radius),
                (rt, b),
                (rt - radius, b),
                (rt - d, b - d),
                90.0,
            ),
            ((l + radius, b), (l, b), (l, b - radius), (l, b - d), 180.0),
        ];
        self.with_brush(color, |brush| unsafe {
            for (a, corner, c, (bx, by), start) in corners {
                let mut path = null_mut();
                gp::GdipCreatePath(gp::FillModeAlternate, &mut path);
                gp::GdipAddPathLine(path, a.0, a.1, corner.0, corner.1);
                gp::GdipAddPathLine(path, corner.0, corner.1, c.0, c.1);
                gp::GdipAddPathArc(path, bx, by, d, d, start, -90.0);
                gp::GdipClosePathFigure(path);
                gp::GdipFillPath(self.0, brush, path);
                gp::GdipDeletePath(path);
            }
        });
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            gp::GdipDeleteGraphics(self.0);
        }
    }
}
